//! Points, rectangles and affine matrices in PDF user space (1/72 inch).

use crate::object::Object;
use serde::{Deserialize, Serialize};

/// A point in user space.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate (up is positive).
    pub y: f64,
}

impl Point {
    /// Creates a point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle given by two opposite corners, as in a PDF
/// `/MediaBox`. Normalised so `x0 <= x1` and `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    /// Left.
    pub x0: f64,
    /// Bottom.
    pub y0: f64,
    /// Right.
    pub x1: f64,
    /// Top.
    pub y1: f64,
}

impl Rect {
    /// Creates a rectangle from any two opposite corners.
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }
    /// Creates a rectangle from origin and size.
    pub fn from_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self::new(x, y, x + w, y + h)
    }
    /// Width.
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    /// Height.
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
    /// Parses a PDF rectangle array. Returns `None` if it is not four numbers.
    pub fn from_object(o: &Object) -> Option<Self> {
        let a = o.as_array()?;
        if a.len() < 4 {
            return None;
        }
        let v: Option<Vec<f64>> = a.iter().take(4).map(Object::as_f64).collect();
        let v = v?;
        if v.iter().any(|x| !x.is_finite()) {
            return None;
        }
        Some(Self::new(v[0], v[1], v[2], v[3]))
    }
    /// Converts to a PDF array object.
    pub fn to_object(&self) -> Object {
        Object::Array(vec![
            Object::Real(self.x0),
            Object::Real(self.y0),
            Object::Real(self.x1),
            Object::Real(self.y1),
        ])
    }
    /// Translates by `(dx, dy)`.
    pub fn translate(&self, dx: f64, dy: f64) -> Self {
        Self {
            x0: self.x0 + dx,
            y0: self.y0 + dy,
            x1: self.x1 + dx,
            y1: self.y1 + dy,
        }
    }
    /// The centre point.
    pub fn center(&self) -> Point {
        Point::new((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    /// Smallest rectangle containing both.
    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
    /// The overlap, or `None` when the rectangles do not touch.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (r.x1 > r.x0 && r.y1 > r.y0).then_some(r)
    }
    /// Whether the rectangles overlap (touching edges do not count).
    pub fn intersects(&self, other: &Rect) -> bool {
        self.intersection(other).is_some()
    }
    /// Whether `other` lies entirely inside `self`.
    pub fn contains(&self, other: &Rect) -> bool {
        other.x0 >= self.x0 && other.y0 >= self.y0 && other.x1 <= self.x1 && other.y1 <= self.y1
    }
    /// Whether the point lies inside (edges included).
    pub fn contains_point(&self, p: Point) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
    /// Grows every side by `d` (shrinks when negative).
    pub fn expand(&self, d: f64) -> Rect {
        Rect::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }
    /// Bounding box of the rectangle after transforming its corners.
    pub fn transform(&self, m: &Matrix) -> Rect {
        let ps = [
            m.apply(Point::new(self.x0, self.y0)),
            m.apply(Point::new(self.x1, self.y0)),
            m.apply(Point::new(self.x0, self.y1)),
            m.apply(Point::new(self.x1, self.y1)),
        ];
        let mut r = Rect::new(ps[0].x, ps[0].y, ps[0].x, ps[0].y);
        for p in &ps[1..] {
            r.x0 = r.x0.min(p.x);
            r.y0 = r.y0.min(p.y);
            r.x1 = r.x1.max(p.x);
            r.y1 = r.y1.max(p.y);
        }
        r
    }
    /// Bounding box of a set of points, or `None` when empty.
    pub fn bounds(points: impl IntoIterator<Item = Point>) -> Option<Rect> {
        let mut it = points.into_iter();
        let first = it.next()?;
        let mut r = Rect::new(first.x, first.y, first.x, first.y);
        for p in it {
            r.x0 = r.x0.min(p.x);
            r.y0 = r.y0.min(p.y);
            r.x1 = r.x1.max(p.x);
            r.y1 = r.y1.max(p.y);
        }
        Some(r)
    }
}

/// A 2-D affine matrix `[a b c d e f]` as used by the `cm` and `Tm` operators.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    /// Scale x / rotation.
    pub a: f64,
    /// Skew / rotation.
    pub b: f64,
    /// Skew / rotation.
    pub c: f64,
    /// Scale y / rotation.
    pub d: f64,
    /// Translate x.
    pub e: f64,
    /// Translate y.
    pub f: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    /// The identity matrix.
    pub const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Creates a matrix from its six coefficients.
    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }
    /// Translation.
    pub fn translate(tx: f64, ty: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }
    /// Scale.
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }
    /// Counter-clockwise rotation in degrees.
    pub fn rotate_deg(deg: f64) -> Self {
        let r = deg.to_radians();
        let (s, c) = r.sin_cos();
        Self::new(c, s, -s, c, 0.0, 0.0)
    }
    /// `self` followed by `other` (i.e. `self × other` in PDF row-vector
    /// convention).
    pub fn then(&self, other: &Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }
    /// The inverse, or `None` when the matrix is singular.
    pub fn invert(&self) -> Option<Matrix> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Matrix {
            a,
            b,
            c,
            d,
            e: -(self.e * a + self.f * c),
            f: -(self.e * b + self.f * d),
        })
    }
    /// The rotation/scale part with the translation removed.
    pub fn linear(&self) -> Matrix {
        Matrix::new(self.a, self.b, self.c, self.d, 0.0, 0.0)
    }
    /// Transforms a point.
    pub fn apply(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }
    /// Converts to a PDF array object.
    pub fn to_object(&self) -> Object {
        Object::Array(
            [self.a, self.b, self.c, self.d, self.e, self.f]
                .iter()
                .map(|&v| Object::Real(v))
                .collect(),
        )
    }
    /// Parses a six-number array.
    pub fn from_object(o: &Object) -> Option<Self> {
        let a = o.as_array()?;
        if a.len() < 6 {
            return None;
        }
        let v: Option<Vec<f64>> = a.iter().take(6).map(Object::as_f64).collect();
        let v = v?;
        Some(Self::new(v[0], v[1], v[2], v[3], v[4], v[5]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_normalises() {
        let r = Rect::new(10.0, 20.0, 0.0, 0.0);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (0.0, 0.0, 10.0, 20.0));
        assert_eq!(r.width(), 10.0);
    }

    #[test]
    fn matrix_compose() {
        let m = Matrix::scale(2.0, 2.0).then(&Matrix::translate(5.0, 5.0));
        let p = m.apply(Point::new(1.0, 1.0));
        assert_eq!(p, Point::new(7.0, 7.0));
        let r = Matrix::rotate_deg(90.0).apply(Point::new(1.0, 0.0));
        assert!((r.x).abs() < 1e-9 && (r.y - 1.0).abs() < 1e-9);
        let inv = m.invert().unwrap();
        let back = inv.apply(p);
        assert!((back.x - 1.0).abs() < 1e-9 && (back.y - 1.0).abs() < 1e-9);
        assert!(Matrix::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0).invert().is_none());
    }

    #[test]
    fn rect_ops() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 20.0, 20.0);
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 20.0, 20.0));
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 10.0, 10.0)));
        assert!(!a.intersects(&Rect::new(10.0, 0.0, 20.0, 10.0)));
        assert!(a.contains(&Rect::new(1.0, 1.0, 2.0, 2.0)));
        let t = a.transform(&Matrix::rotate_deg(90.0));
        assert!((t.x0 + 10.0).abs() < 1e-9 && (t.x1).abs() < 1e-9);
    }
}
