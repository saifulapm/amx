//! Cell-grid rectangles and the exact-tiling split.

/// An axis-aligned rectangle in terminal cells.
///
/// `(x, y)` is the top-left corner; `w`/`h` may be zero, which represents a
/// pane that a resize or an extreme split ratio has squeezed out of visible
/// space. A zero-size rect still tiles correctly: it simply contributes no
/// cells, so it can neither overlap nor gap its neighbours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    /// Left edge, in cells from the workspace origin.
    pub x: u16,
    /// Top edge, in cells from the workspace origin.
    pub y: u16,
    /// Width in cells.
    pub w: u16,
    /// Height in cells.
    pub h: u16,
}

impl Rect {
    /// A rect of `w` by `h` cells at the origin.
    #[must_use]
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    /// Split this rect into two that exactly tile it, along `axis`.
    ///
    /// `ratio` (clamped to `0.0..=1.0`) is the share of the split dimension
    /// given to the first rect; the second gets the exact remainder, so
    /// `first.w + second.w == self.w` (or the `h` equivalent) always holds —
    /// the tiling invariant is a consequence of subtraction, not of rounding
    /// working out.
    #[must_use]
    pub fn split(self, axis: Axis, ratio: f32) -> (Self, Self) {
        let ratio = ratio.clamp(0.0, 1.0);
        match axis {
            Axis::X => {
                let first_w = ((f32::from(self.w) * ratio).round() as u16).min(self.w);
                let second_w = self.w - first_w;
                (
                    Self::new(self.x, self.y, first_w, self.h),
                    Self::new(self.x + first_w, self.y, second_w, self.h),
                )
            }
            Axis::Y => {
                let first_h = ((f32::from(self.h) * ratio).round() as u16).min(self.h);
                let second_h = self.h - first_h;
                (
                    Self::new(self.x, self.y, self.w, first_h),
                    Self::new(self.x, self.y + first_h, self.w, second_h),
                )
            }
        }
    }

    /// The rect's centre point, rounded toward the origin.
    #[must_use]
    pub const fn center(&self) -> (i32, i32) {
        (
            self.x as i32 + self.w as i32 / 2,
            self.y as i32 + self.h as i32 / 2,
        )
    }
}

/// Which dimension a split divides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    /// Divides width: the two children sit side by side.
    X,
    /// Divides height: the two children stack top and bottom.
    Y,
}

/// A compass direction: which side a new pane lands on, or which way focus
/// moves. Shared between splits and `hjkl` movement because both name the
/// same four neighbours of a rect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// `h`
    Left,
    /// `j`
    Down,
    /// `k`
    Up,
    /// `l`
    Right,
}

impl Direction {
    /// The axis a split in this direction divides.
    #[must_use]
    pub const fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::X,
            Self::Up | Self::Down => Axis::Y,
        }
    }

    /// Whether a split in this direction puts the *new* pane in the first
    /// (top/left) slot rather than the second (bottom/right) one.
    #[must_use]
    pub const fn new_pane_is_first(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }
}
