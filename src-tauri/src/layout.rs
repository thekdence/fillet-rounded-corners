//! Where windows go. Pure geometry, no Win32, so it's easy to test.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { x: left, y: top, w: right - left, h: bottom - top }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// True when every edge is within `tol` pixels of `other`'s.
    pub fn near(&self, other: &Rect, tol: i32) -> bool {
        (self.x - other.x).abs() <= tol
            && (self.y - other.y).abs() <= tol
            && (self.right() - other.right()).abs() <= tol
            && (self.bottom() - other.bottom()).abs() <= tol
    }

    /// True when `self` lies entirely inside `other`.
    pub fn is_within(&self, other: &Rect) -> bool {
        other.x <= self.x && other.y <= self.y && other.right() >= self.right() && other.bottom() >= self.bottom()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Full,
    Center,
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftThird,
    CenterThird,
    RightThird,
    LeftTwoThirds,
    RightTwoThirds,
}

impl Layout {
    /// Layouts that match a Windows snap zone. `Center` is Fillet's own.
    const SNAP_ZONES: [Layout; 14] = [
        Layout::Full,
        Layout::LeftHalf,
        Layout::RightHalf,
        Layout::TopHalf,
        Layout::BottomHalf,
        Layout::TopLeft,
        Layout::TopRight,
        Layout::BottomLeft,
        Layout::BottomRight,
        Layout::LeftThird,
        Layout::CenterThird,
        Layout::RightThird,
        Layout::LeftTwoThirds,
        Layout::RightTwoThirds,
    ];

    /// The rectangle this layout fills inside `work`, keeping `margin` from
    /// the screen edges and `gap` between neighbours.
    pub fn bounds(self, work: Rect, margin: i32, gap: i32) -> Rect {
        let (m, g) = (margin as f64, gap as f64);
        let (ww, wh) = (work.w as f64, work.h as f64);
        let r = |v: f64| v.round() as i32;

        // Work out the edges first, so every margin and gap comes out exact
        // whatever the rounding.
        let (left, right) = (work.x + margin, work.right() - margin);
        let (top, bottom) = (work.y + margin, work.bottom() - margin);
        let mid_x = work.x + r(ww / 2.0 - g / 2.0);
        let mid_y = work.y + r(wh / 2.0 - g / 2.0);
        let col = (ww - m * 2.0 - g * 2.0) / 3.0;
        let third_1 = work.x + r(m + col);
        let third_2 = work.x + r(m + col * 2.0 + g);

        let span = |l: i32, t: i32, r: i32, b: i32| Rect::from_edges(l, t, r, b);
        match self {
            Layout::Full => span(left, top, right, bottom),
            Layout::LeftHalf => span(left, top, mid_x, bottom),
            Layout::RightHalf => span(mid_x + gap, top, right, bottom),
            Layout::TopHalf => span(left, top, right, mid_y),
            Layout::BottomHalf => span(left, mid_y + gap, right, bottom),
            Layout::TopLeft => span(left, top, mid_x, mid_y),
            Layout::TopRight => span(mid_x + gap, top, right, mid_y),
            Layout::BottomLeft => span(left, mid_y + gap, mid_x, bottom),
            Layout::BottomRight => span(mid_x + gap, mid_y + gap, right, bottom),
            Layout::LeftThird => span(left, top, third_1, bottom),
            Layout::CenterThird => span(third_1 + gap, top, third_2, bottom),
            Layout::RightThird => span(third_2 + gap, top, right, bottom),
            Layout::LeftTwoThirds => span(left, top, third_2, bottom),
            Layout::RightTwoThirds => span(third_1 + gap, top, right, bottom),
            Layout::Center => Rect {
                x: r(work.x as f64 + m + ww * 0.1),
                y: r(work.y as f64 + m + wh * 0.1),
                w: r(ww * 0.8 - m * 2.0),
                h: r(wh * 0.8 - m * 2.0),
            },
        }
    }

    /// The layout a window most likely belongs to, judged loosely by its size
    /// and position. Used when you ask Fillet to tidy a window by hand.
    pub fn guess(frame: Rect, work: Rect, maximized: bool) -> Layout {
        if maximized || work.w <= 0 || work.h <= 0 {
            return Layout::Full;
        }
        let rel_x = (frame.x - work.x) as f64 / work.w as f64;
        let rel_y = (frame.y - work.y) as f64 / work.h as f64;
        let rel_w = frame.w as f64 / work.w as f64;
        let rel_h = frame.h as f64 / work.h as f64;
        let cx = rel_x + rel_w / 2.0;
        let cy = rel_y + rel_h / 2.0;
        let left = cx < 0.5;
        let upper = cy < 0.5;
        let quarter = |left: bool, upper: bool| match (left, upper) {
            (true, true) => Layout::TopLeft,
            (false, true) => Layout::TopRight,
            (true, false) => Layout::BottomLeft,
            (false, false) => Layout::BottomRight,
        };
        let side_half = if left { Layout::LeftHalf } else { Layout::RightHalf };

        if (0.65..=0.85).contains(&rel_w)
            && (0.65..=0.88).contains(&rel_h)
            && (cx - 0.5).abs() <= 0.08
            && (cy - 0.5).abs() <= 0.08
        {
            return Layout::Center;
        }
        if rel_w >= 0.78 && rel_h >= 0.78 {
            return Layout::Full;
        }
        if (0.20..0.42).contains(&rel_w) && rel_h >= 0.70 {
            return if cx < 0.38 {
                Layout::LeftThird
            } else if cx > 0.62 {
                Layout::RightThird
            } else {
                Layout::CenterThird
            };
        }
        if (0.42..=0.60).contains(&rel_w) {
            if rel_h >= 0.70 {
                return side_half;
            }
            if (0.35..0.70).contains(&rel_h) {
                return quarter(left, upper);
            }
        }
        if rel_w > 0.60 && rel_w < 0.78 && rel_h >= 0.70 {
            return if left { Layout::LeftTwoThirds } else { Layout::RightTwoThirds };
        }
        if rel_w >= 0.70 && (0.35..=0.65).contains(&rel_h) {
            return if upper { Layout::TopHalf } else { Layout::BottomHalf };
        }
        if rel_w >= 0.70 && rel_h >= 0.70 {
            return Layout::Full;
        }
        if rel_h >= 0.60 {
            return side_half;
        }
        quarter(left, upper)
    }

    /// The snap zone a window sits in, if it lines up with one either as
    /// Windows placed it (no gaps) or as Fillet placed it (with gaps).
    /// Used for automatic tidying, so it only matches tight fits.
    pub fn snapped(frame: Rect, work: Rect, margin: i32, gap: i32, tol: i32) -> Option<Layout> {
        Self::SNAP_ZONES.into_iter().find(|l| {
            frame.near(&l.bounds(work, 0, 0), tol) || frame.near(&l.bounds(work, margin, gap), tol)
        })
    }

    /// The layout a window already exactly occupies, gaps included.
    pub fn current(frame: Rect, work: Rect, margin: i32, gap: i32) -> Option<Layout> {
        const ALL: [Layout; 15] = [
            Layout::Full,
            Layout::Center,
            Layout::LeftHalf,
            Layout::RightHalf,
            Layout::TopHalf,
            Layout::BottomHalf,
            Layout::TopLeft,
            Layout::TopRight,
            Layout::BottomLeft,
            Layout::BottomRight,
            Layout::LeftThird,
            Layout::CenterThird,
            Layout::RightThird,
            Layout::LeftTwoThirds,
            Layout::RightTwoThirds,
        ];
        ALL.into_iter().find(|l| frame.near(&l.bounds(work, margin, gap), 2))
    }

    /// Pressing "snap left" again cycles half → two thirds → one third.
    pub fn next_left(current: Option<Layout>) -> Layout {
        match current {
            Some(Layout::LeftHalf) => Layout::LeftTwoThirds,
            Some(Layout::LeftTwoThirds) => Layout::LeftThird,
            _ => Layout::LeftHalf,
        }
    }

    pub fn next_right(current: Option<Layout>) -> Layout {
        match current {
            Some(Layout::RightHalf) => Layout::RightTwoThirds,
            Some(Layout::RightTwoThirds) => Layout::RightThird,
            _ => Layout::RightHalf,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { x: 0, y: 0, w: 1920, h: 1032 };

    #[test]
    fn halves_leave_margin_and_gap() {
        let l = Layout::LeftHalf.bounds(WORK, 15, 10);
        let r = Layout::RightHalf.bounds(WORK, 15, 10);
        assert_eq!(l, Rect { x: 15, y: 15, w: 940, h: 1002 });
        assert_eq!(r.x - l.right(), 10);
        assert_eq!(WORK.right() - r.right(), 15);
    }

    #[test]
    fn thirds_tile_the_screen() {
        let a = Layout::LeftThird.bounds(WORK, 15, 10);
        let b = Layout::CenterThird.bounds(WORK, 15, 10);
        let c = Layout::RightThird.bounds(WORK, 15, 10);
        assert_eq!(b.x - a.right(), 10);
        assert_eq!(c.x - b.right(), 10);
        assert_eq!(WORK.right() - c.right(), 15);
        assert!((a.w - b.w).abs() <= 1 && (b.w - c.w).abs() <= 1);
        let two = Layout::LeftTwoThirds.bounds(WORK, 15, 10);
        assert_eq!(two.right(), b.right());
    }

    #[test]
    fn edges_are_exact_with_odd_sizes() {
        // 125% scaling: 19px margin, 13px gap on a 2560px-wide screen.
        let work = Rect { x: 0, y: 0, w: 2560, h: 1380 };
        let l = Layout::LeftHalf.bounds(work, 19, 13);
        let r = Layout::RightHalf.bounds(work, 19, 13);
        assert_eq!(r.x - l.right(), 13);
        assert_eq!(work.right() - r.right(), 19);
        let q = Layout::BottomRight.bounds(work, 19, 13);
        assert_eq!(work.bottom() - q.bottom(), 19);
        assert!((l.w - r.w).abs() <= 1);
    }

    #[test]
    fn respects_work_area_offset() {
        let second = Rect { x: 1920, y: 40, w: 2560, h: 1400 };
        let full = Layout::Full.bounds(second, 15, 10);
        assert_eq!(full, Rect { x: 1935, y: 55, w: 2530, h: 1370 });
    }

    #[test]
    fn guesses_common_layouts() {
        let g = |l: Layout| Layout::guess(l.bounds(WORK, 0, 0), WORK, false);
        for l in [
            Layout::Full,
            Layout::LeftHalf,
            Layout::RightHalf,
            Layout::TopLeft,
            Layout::BottomRight,
            Layout::LeftThird,
            Layout::CenterThird,
            Layout::RightThird,
            Layout::LeftTwoThirds,
            Layout::RightTwoThirds,
            Layout::TopHalf,
            Layout::BottomHalf,
        ] {
            assert_eq!(g(l), l, "{l:?}");
        }
        assert_eq!(Layout::guess(Layout::Center.bounds(WORK, 15, 10), WORK, false), Layout::Center);
        assert_eq!(Layout::guess(Rect { x: 300, y: 200, w: 400, h: 300 }, WORK, true), Layout::Full);
    }

    #[test]
    fn snapped_only_matches_tight_fits() {
        let snapped = Layout::RightHalf.bounds(WORK, 0, 0);
        assert_eq!(Layout::snapped(snapped, WORK, 15, 10, 12), Some(Layout::RightHalf));
        let padded = Layout::TopLeft.bounds(WORK, 15, 10);
        assert_eq!(Layout::snapped(padded, WORK, 15, 10, 12), Some(Layout::TopLeft));
        let floating = Rect { x: 200, y: 150, w: 900, h: 700 };
        assert_eq!(Layout::snapped(floating, WORK, 15, 10, 12), None);
    }

    #[test]
    fn snap_left_cycles() {
        let at = |l: Layout| Layout::current(l.bounds(WORK, 15, 10), WORK, 15, 10);
        assert_eq!(Layout::next_left(None), Layout::LeftHalf);
        assert_eq!(Layout::next_left(at(Layout::LeftHalf)), Layout::LeftTwoThirds);
        assert_eq!(Layout::next_left(at(Layout::LeftTwoThirds)), Layout::LeftThird);
        assert_eq!(Layout::next_left(at(Layout::LeftThird)), Layout::LeftHalf);
        assert_eq!(Layout::next_right(at(Layout::LeftHalf)), Layout::RightHalf);
    }
}
