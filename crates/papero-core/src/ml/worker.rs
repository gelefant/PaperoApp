use crate::error::{CoreError, Result};
use crate::ml::embedder::OnnxEmbedder;
use crate::ml::registry::{model_specs_for_profile, ModelProfile, ModelRole};
use crate::ml::reranker::OnnxReranker;
use serde::{Deserialize, Serialize};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Debug, Serialize, Deserialize)]
struct WorkerInit {
    role: ModelRole,
    models_dir: PathBuf,
    profile: ModelProfile,
    ort_lib_path: Option<PathBuf>,
    intra_threads: usize,
}

#[derive(Debug, Serialize, Deserialize)]
enum WorkerRequest {
    EmbedQuery { query: String },
    EmbedPassages { passages: Vec<String> },
    RerankPairs { query: String, passages: Vec<String> },
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
enum WorkerResponse {
    Ready,
    Vectors { vectors: Vec<Vec<f32>> },
    Scores { scores: Vec<f32> },
    Error { message: String },
}

fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "worker frame exceeds 4 GB"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(io::Error::other)
}

/// Persistent child-process host for a single loaded ONNX model (`Embedder` or `Reranker`).
///
/// Keeping `libonnxruntime` inside a disposable subprocess guarantees that unloading on
/// idle timeout returns 100% of ONNX Runtime weights and C++ heap allocations to the OS,
/// keeping the main desktop process well under the `<= 250 MB` idle footprint budget.
pub struct MlWorkerClient {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    stdout: BufReader<ChildStdout>,
}

impl MlWorkerClient {
    pub fn spawn(
        role: ModelRole,
        models_dir: &Path,
        profile: ModelProfile,
        ort_lib_path: Option<&Path>,
        intra_threads: usize,
    ) -> Result<Self> {
        let exe = std::env::current_exe()
            .map_err(|e| CoreError::Config(format!("cannot locate executable for ml-worker: {e}")))?;

        let mut cmd = Command::new(&exe);
        cmd.arg("--ml-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::Config(format!("failed spawning ml-worker subprocess: {e}")))?;

        let stdin_pipe = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Config("ml-worker missing piped stdin".into()))?;
        let stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Config("ml-worker missing piped stdout".into()))?;

        let mut stdin = BufWriter::new(stdin_pipe);
        let mut stdout = BufReader::new(stdout_pipe);

        let init = WorkerInit {
            role,
            models_dir: models_dir.to_path_buf(),
            profile,
            ort_lib_path: ort_lib_path.map(Path::to_path_buf),
            intra_threads: intra_threads.max(1),
        };
        if let Err(e) = write_frame(&mut stdin, &init) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CoreError::Config(format!("failed writing ml-worker init: {e}")));
        }

        match read_frame::<_, WorkerResponse>(&mut stdout) {
            Ok(WorkerResponse::Ready) => Ok(Self {
                child,
                stdin: Some(stdin),
                stdout,
            }),
            Ok(WorkerResponse::Error { message }) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(CoreError::Config(message))
            }
            Ok(_) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(CoreError::Config("unexpected initial response from ml-worker".into()))
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(CoreError::Config(format!("ml-worker failed during startup: {e}")))
            }
        }
    }

    fn exchange(&mut self, req: &WorkerRequest) -> Result<WorkerResponse> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| CoreError::Config("ml-worker channel already closed".into()))?;
        write_frame(stdin, req)
            .map_err(|e| CoreError::Config(format!("ml-worker write failed: {e}")))?;
        let resp: WorkerResponse = read_frame(&mut self.stdout)
            .map_err(|e| CoreError::Config(format!("ml-worker read failed: {e}")))?;
        if let WorkerResponse::Error { message } = resp {
            return Err(CoreError::Config(message));
        }
        Ok(resp)
    }

    pub fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        match self.exchange(&WorkerRequest::EmbedQuery {
            query: query.to_string(),
        })? {
            WorkerResponse::Vectors { mut vectors } => Ok(vectors.pop().unwrap_or_default()),
            _ => Err(CoreError::Config("unexpected ml-worker response for EmbedQuery".into())),
        }
    }

    pub fn embed_passages(&mut self, passages: &[&str]) -> Result<Vec<Vec<f32>>> {
        if passages.is_empty() {
            return Ok(Vec::new());
        }
        let owned: Vec<String> = passages.iter().map(|s| (*s).to_string()).collect();
        match self.exchange(&WorkerRequest::EmbedPassages { passages: owned })? {
            WorkerResponse::Vectors { vectors } => Ok(vectors),
            _ => Err(CoreError::Config(
                "unexpected ml-worker response for EmbedPassages".into(),
            )),
        }
    }

    pub fn score_pairs(&mut self, query: &str, passages: &[&str]) -> Result<Vec<f32>> {
        if passages.is_empty() {
            return Ok(Vec::new());
        }
        let owned: Vec<String> = passages.iter().map(|s| (*s).to_string()).collect();
        match self.exchange(&WorkerRequest::RerankPairs {
            query: query.to_string(),
            passages: owned,
        })? {
            WorkerResponse::Scores { scores } => Ok(scores),
            _ => Err(CoreError::Config("unexpected ml-worker response for RerankPairs".into())),
        }
    }
}

impl Drop for MlWorkerClient {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = write_frame(&mut stdin, &WorkerRequest::Shutdown);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Checks spec lookup cleanly in-process without needing an IPC roundtrip.
pub fn find_spec(profile: ModelProfile, role: ModelRole) -> Result<crate::ml::registry::ModelSpec> {
    model_specs_for_profile(profile)
        .into_iter()
        .find(|s| s.role == role)
        .ok_or_else(|| CoreError::Config(format!("missing model spec for {:?}", role)))
}

fn lower_worker_process_priority() {
    let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
    #[cfg(target_os = "macos")]
    {
        // QOS_CLASS_UTILITY (0x11) tells XNU to schedule this worker and the intra-op pthreads
        // spawned by ONNX Runtime onto Efficiency cores / low-power P-states rather than blasting fans.
        const QOS_CLASS_UTILITY: u32 = 0x11;
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        // SAFETY: Standard libsystem_pthread call setting the calling thread's QoS class.
        unsafe {
            let _ = pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
    }
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn nice(inc: i32) -> i32;
        }
        // SAFETY: Standard libc `nice` call lowering background worker scheduling priority.
        unsafe {
            let _ = nice(10);
        }
    }
}

/// Stdio worker loop invoked via `<binary> --ml-worker`.
pub fn run_ml_worker_stdio() -> Result<()> {
    lower_worker_process_priority();

    let stdin_raw = io::stdin();
    let stdout_raw = io::stdout();
    let mut reader = BufReader::new(stdin_raw.lock());
    let mut writer = BufWriter::new(stdout_raw.lock());

    let init: WorkerInit = match read_frame(&mut reader) {
        Ok(i) => i,
        Err(e) => {
            let _ = write_frame(
                &mut writer,
                &WorkerResponse::Error {
                    message: format!("invalid ml-worker init frame: {e}"),
                },
            );
            return Err(CoreError::Config(format!("invalid ml-worker init: {e}")));
        }
    };

    enum LoadedModel {
        Embedder(OnnxEmbedder),
        Reranker(OnnxReranker),
    }

    let mut model = match init.role {
        ModelRole::Embedder => match OnnxEmbedder::load(
            &init.models_dir,
            init.profile,
            init.ort_lib_path.as_deref(),
            init.intra_threads,
        ) {
            Ok(m) => LoadedModel::Embedder(m),
            Err(e) => {
                let _ = write_frame(
                    &mut writer,
                    &WorkerResponse::Error {
                        message: e.to_string(),
                    },
                );
                return Err(e);
            }
        },
        ModelRole::Reranker => match OnnxReranker::load(
            &init.models_dir,
            init.profile,
            init.ort_lib_path.as_deref(),
            init.intra_threads,
        ) {
            Ok(m) => LoadedModel::Reranker(m),
            Err(e) => {
                let _ = write_frame(
                    &mut writer,
                    &WorkerResponse::Error {
                        message: e.to_string(),
                    },
                );
                return Err(e);
            }
        },
    };

    write_frame(&mut writer, &WorkerResponse::Ready)
        .map_err(|e| CoreError::Config(format!("ml-worker failed sending Ready: {e}")))?;

    loop {
        let req: WorkerRequest = match read_frame(&mut reader) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(CoreError::Config(format!("ml-worker read request error: {e}")));
            }
        };

        let resp = match req {
            WorkerRequest::Shutdown => break,
            WorkerRequest::EmbedQuery { query } => match &mut model {
                LoadedModel::Embedder(emb) => match emb.embed_query(&query) {
                    Ok(v) => WorkerResponse::Vectors { vectors: vec![v] },
                    Err(e) => WorkerResponse::Error {
                        message: e.to_string(),
                    },
                },
                LoadedModel::Reranker(_) => WorkerResponse::Error {
                    message: "EmbedQuery sent to reranker worker".into(),
                },
            },
            WorkerRequest::EmbedPassages { passages } => match &mut model {
                LoadedModel::Embedder(emb) => {
                    let refs: Vec<&str> = passages.iter().map(String::as_str).collect();
                    match emb.embed_passages(&refs) {
                        Ok(vectors) => WorkerResponse::Vectors { vectors },
                        Err(e) => WorkerResponse::Error {
                            message: e.to_string(),
                        },
                    }
                }
                LoadedModel::Reranker(_) => WorkerResponse::Error {
                    message: "EmbedPassages sent to reranker worker".into(),
                },
            },
            WorkerRequest::RerankPairs { query, passages } => match &mut model {
                LoadedModel::Reranker(rr) => {
                    let refs: Vec<&str> = passages.iter().map(String::as_str).collect();
                    match rr.score_pairs(&query, &refs) {
                        Ok(scores) => WorkerResponse::Scores { scores },
                        Err(e) => WorkerResponse::Error {
                            message: e.to_string(),
                        },
                    }
                }
                LoadedModel::Embedder(_) => WorkerResponse::Error {
                    message: "RerankPairs sent to embedder worker".into(),
                },
            },
        };

        write_frame(&mut writer, &resp)
            .map_err(|e| CoreError::Config(format!("ml-worker failed writing response: {e}")))?;
    }

    Ok(())
}
