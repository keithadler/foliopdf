//! Page-level types: sizes, rotation and the [`PageInfo`] summary.

use crate::geometry::Rect;
use crate::object::ObjRef;
use serde::{Deserialize, Serialize};

/// Page rotation as stored in `/Rotate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rotation {
    /// No rotation.
    #[default]
    #[serde(rename = "0")]
    R0,
    /// 90° clockwise.
    #[serde(rename = "90")]
    R90,
    /// 180°.
    #[serde(rename = "180")]
    R180,
    /// 270° clockwise (90° counter-clockwise).
    #[serde(rename = "270")]
    R270,
}

impl Rotation {
    /// Normalises any multiple of 90 (negative allowed). Other values round
    /// to the nearest quarter turn.
    pub fn from_degrees(deg: i64) -> Self {
        let q = ((deg as f64 / 90.0).round() as i64).rem_euclid(4);
        match q {
            0 => Rotation::R0,
            1 => Rotation::R90,
            2 => Rotation::R180,
            _ => Rotation::R270,
        }
    }
    /// Degrees clockwise, 0–270.
    pub fn degrees(self) -> i64 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }
    /// Adds a rotation.
    pub fn plus(self, deg: i64) -> Self {
        Self::from_degrees(self.degrees() + deg)
    }
    /// Whether width and height swap when displayed.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::R90 | Rotation::R270)
    }
}

/// A page size in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    /// Width in points.
    pub width: f64,
    /// Height in points.
    pub height: f64,
}

impl PageSize {
    /// US Letter, 8.5 × 11 in.
    pub const LETTER: PageSize = PageSize {
        width: 612.0,
        height: 792.0,
    };
    /// US Legal, 8.5 × 14 in.
    pub const LEGAL: PageSize = PageSize {
        width: 612.0,
        height: 1008.0,
    };
    /// Tabloid / Ledger, 11 × 17 in.
    pub const TABLOID: PageSize = PageSize {
        width: 792.0,
        height: 1224.0,
    };
    /// ISO A3.
    pub const A3: PageSize = PageSize {
        width: 841.89,
        height: 1190.55,
    };
    /// ISO A4.
    pub const A4: PageSize = PageSize {
        width: 595.28,
        height: 841.89,
    };
    /// ISO A5.
    pub const A5: PageSize = PageSize {
        width: 419.53,
        height: 595.28,
    };

    /// Creates a custom size.
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
    /// Swaps width and height.
    pub fn landscape(self) -> Self {
        Self {
            width: self.height.max(self.width),
            height: self.height.min(self.width),
        }
    }
    /// Looks up a named size (`letter`, `a4`, `legal`, `a3`, `a5`, `tabloid`),
    /// optionally suffixed with `-landscape`.
    pub fn by_name(name: &str) -> Option<Self> {
        let n = name.to_ascii_lowercase();
        let (base, land) = match n.strip_suffix("-landscape") {
            Some(b) => (b.to_string(), true),
            None => (n.clone(), false),
        };
        let s = match base.as_str() {
            "letter" => Self::LETTER,
            "legal" => Self::LEGAL,
            "tabloid" | "ledger" => Self::TABLOID,
            "a3" => Self::A3,
            "a4" => Self::A4,
            "a5" => Self::A5,
            _ => return None,
        };
        Some(if land { s.landscape() } else { s })
    }
    /// The media box for this size with its origin at (0, 0).
    pub fn rect(self) -> Rect {
        Rect::new(0.0, 0.0, self.width, self.height)
    }
}

/// Summary of a page's geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    /// Zero-based page index.
    pub index: usize,
    /// The page object.
    #[serde(skip)]
    pub obj: ObjRef,
    /// `/MediaBox` (inherited if necessary; Letter when missing).
    pub media_box: Rect,
    /// `/CropBox` if present.
    pub crop_box: Option<Rect>,
    /// `/Rotate`.
    pub rotation: Rotation,
}

impl PageInfo {
    /// The visible area: crop box if present, else media box.
    pub fn visible_box(&self) -> Rect {
        self.crop_box.unwrap_or(self.media_box)
    }
    /// Displayed width after rotation.
    pub fn display_width(&self) -> f64 {
        let b = self.visible_box();
        if self.rotation.swaps_axes() {
            b.height()
        } else {
            b.width()
        }
    }
    /// Displayed height after rotation.
    pub fn display_height(&self) -> f64 {
        let b = self.visible_box();
        if self.rotation.swaps_axes() {
            b.width()
        } else {
            b.height()
        }
    }
    /// Matrix mapping *display* coordinates (origin bottom-left of the page
    /// as the viewer shows it, x right, y up) to user space. Drawing through
    /// this matrix makes stamps appear upright on rotated pages.
    pub fn display_to_user(&self) -> crate::geometry::Matrix {
        use crate::geometry::Matrix;
        let b = self.visible_box();
        let (w, h) = (b.width(), b.height());
        match self.rotation {
            Rotation::R0 => Matrix::translate(b.x0, b.y0),
            Rotation::R90 => Matrix::new(0.0, 1.0, -1.0, 0.0, b.x0 + w, b.y0),
            Rotation::R180 => Matrix::new(-1.0, 0.0, 0.0, -1.0, b.x0 + w, b.y0 + h),
            Rotation::R270 => Matrix::new(0.0, -1.0, 1.0, 0.0, b.x0, b.y0 + h),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;

    #[test]
    fn rotation_normalises() {
        assert_eq!(Rotation::from_degrees(-90), Rotation::R270);
        assert_eq!(Rotation::from_degrees(450), Rotation::R90);
        assert_eq!(Rotation::R270.plus(180), Rotation::R90);
    }

    #[test]
    fn sizes() {
        assert_eq!(PageSize::by_name("A4-landscape").unwrap().width, 841.89);
        assert_eq!(PageSize::by_name("nope"), None);
    }

    #[test]
    fn display_mapping() {
        let info = PageInfo {
            index: 0,
            obj: ObjRef::new(1, 0),
            media_box: Rect::new(0.0, 0.0, 200.0, 100.0),
            crop_box: None,
            rotation: Rotation::R90,
        };
        assert_eq!(info.display_width(), 100.0);
        // Display bottom-left is the user bottom-right corner for a 90° page.
        let p = info.display_to_user().apply(Point::new(0.0, 0.0));
        assert_eq!(p, Point::new(200.0, 0.0));
        let q = info.display_to_user().apply(Point::new(100.0, 200.0));
        assert_eq!(q, Point::new(0.0, 100.0));
    }
}
