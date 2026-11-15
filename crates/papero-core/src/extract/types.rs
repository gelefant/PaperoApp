use serde::{Deserialize, Serialize};

/// Bounding box normalized to `[0.0, 1.0]` with origin `(0, 0)` at TOP-LEFT
/// (`x` rightward, `y` downward).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NormalizedRect {
    pub fn from_bottom_left(
        xmin: f32,
        ymin: f32,
        xmax: f32,
        ymax: f32,
        page_w: f32,
        page_h: f32,
    ) -> Option<Self> {
        if page_w <= 0.0 || page_h <= 0.0 || xmax <= xmin || ymax <= ymin {
            return None;
        }
        let x = (xmin / page_w).clamp(0.0, 1.0);
        let y = ((page_h - ymax) / page_h).clamp(0.0, 1.0);
        let w = ((xmax - xmin) / page_w).clamp(0.0, 1.0);
        let h = ((ymax - ymin) / page_h).clamp(0.0, 1.0);
        Some(Self { x, y, w, h })
    }

    pub fn union(&self, other: &Self) -> Self {
        let x1 = self.x.min(other.x);
        let y1 = self.y.min(other.y);
        let x2 = (self.x + self.w).max(other.x + other.w);
        let y2 = (self.y + self.h).max(other.y + other.h);
        Self {
            x: x1,
            y: y1,
            w: (x2 - x1).max(0.0),
            h: (y2 - y1).max(0.0),
        }
    }
}

/// Packs `NormalizedRect` slice into little-endian `[f32; 4]` bytes (`16` bytes per rect).
pub fn pack_bboxes(rects: &[NormalizedRect]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rects.len() * 16);
    for r in rects {
        out.extend_from_slice(&r.x.to_le_bytes());
        out.extend_from_slice(&r.y.to_le_bytes());
        out.extend_from_slice(&r.w.to_le_bytes());
        out.extend_from_slice(&r.h.to_le_bytes());
    }
    out
}

/// Unpacks little-endian `[f32; 4]` bytes into `Vec<NormalizedRect>`.
pub fn unpack_bboxes(blob: &[u8]) -> Vec<NormalizedRect> {
    let (chunks, _) = blob.as_chunks::<16>();
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let x = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let y = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        let w = f32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
        let h = f32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]);
        out.push(NormalizedRect { x, y, w, h });
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedSpan {
    pub text: String,
    pub bbox: NormalizedRect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedPage {
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub width_pts: f32,
    pub height_pts: f32,
    pub spans: Vec<ExtractedSpan>,
    pub raw_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    pub page_ord: Option<u32>,
    pub children: Vec<OutlineItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedDocument {
    pub page_count: u32,
    pub has_text_layer: bool,
    pub pages: Vec<ExtractedPage>,
    pub outline: Vec<OutlineItem>,
    pub xmp_xml: Option<String>,
    pub doc_title: Option<String>,
    pub doc_author: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_left_y_flips_to_top_left() {
        // Page is 100w x 200h. Box near top: ymin=150, ymax=180 -> top_y = (200-180)/200 = 0.10
        let rect = NormalizedRect::from_bottom_left(10.0, 150.0, 90.0, 180.0, 100.0, 200.0)
            .expect("valid rect");
        assert!((rect.x - 0.10).abs() < 1e-6);
        assert!((rect.y - 0.10).abs() < 1e-6);
        assert!((rect.w - 0.80).abs() < 1e-6);
        assert!((rect.h - 0.15).abs() < 1e-6);
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let rects = vec![
            NormalizedRect {
                x: 0.1,
                y: 0.2,
                w: 0.5,
                h: 0.05,
            },
            NormalizedRect {
                x: 0.15,
                y: 0.3,
                w: 0.4,
                h: 0.04,
            },
        ];
        let packed = pack_bboxes(&rects);
        assert_eq!(packed.len(), 32);
        let unpacked = unpack_bboxes(&packed);
        assert_eq!(unpacked, rects);
    }
}
