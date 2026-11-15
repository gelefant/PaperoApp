use papero_core::db::repos::roots::{insert_source_root, NewSourceRoot};
use papero_core::db::Database;
use papero_core::fs::roots::new_root_id;
use papero_core::fs::scanner::rescan_sources;
use std::fs;
use std::time::Instant;

fn main() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = tmp.path().join("bench_5000");
    fs::create_dir_all(&lib_dir).unwrap();
    for s in 0..50 {
        fs::create_dir_all(lib_dir.join(format!("s{s:02}"))).unwrap();
    }
    for i in 0..5000 {
        let p = lib_dir.join(format!("s{:02}/paper_{i:05}.pdf", i % 50));
        fs::write(p, format!("%PDF-1.4 doc {i}")).unwrap();
    }

    let db = Database::open(&tmp.path().join("papero.db"), 4).unwrap();
    {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: new_root_id(),
                label: "Bench".into(),
                root_path: lib_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();
    }

    let _ = rescan_sources(&db, None, None, None).unwrap();
    let start = Instant::now();
    let s2 = rescan_sources(&db, None, None, None).unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "perf_targets::rescan_5000_unchanged: unchanged={} elapsed={:.4}s",
        s2.cnt_unchanged, elapsed
    );
    assert_eq!(s2.cnt_unchanged, 5000);
    assert!(elapsed <= 10.0, "rescan took {elapsed:.3}s > 10.0s target");
}
