//! How far the picture is enlarged and which part of it is in view.
//!
//! All of it is arithmetic on sizes and points, with nothing of AppKit or
//! the player in it: the fingers say where, this says what the picture
//! does, and the screen tells the player.

/// A width and a height, in points or in the picture's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// A place in the view, in points from its bottom left corner, or a
/// distance across it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The picture's size and place in the view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    /// How many times larger the picture is than the size that fits the
    /// view, from 1 to [`Zoom::MOST`].
    scale: f64,
    /// The point of the picture that is in the middle of the view, as
    /// fractions of its width from the left and of its height from the
    /// bottom.
    focus: Point,
}

impl Zoom {
    /// The whole picture, as large as the view lets it be.
    pub const FIT: Self = Self { scale: 1.0, focus: Point { x: 0.5, y: 0.5 } };

    /// Past this the picture is a few blurred shapes.
    const MOST: f64 = 8.0;

    /// The picture enlarged until it covers the view, which crops what
    /// does not have the view's shape: the bars of a 2.39 to 1 film in a
    /// 16 to 9 window, or of a 4 to 3 one.
    pub fn fill(view: Size, picture: Size) -> Self {
        let fitted = fitted(view, picture);
        let scale = (view.width / fitted.width).max(view.height / fitted.height);
        Self { scale: scale.clamp(1.0, Self::MOST), ..Self::FIT }
    }

    pub fn scale(self) -> f64 {
        self.scale
    }

    pub fn is_fit(self) -> bool {
        self.scale <= 1.0
    }

    /// Where the scale lies between fit and the most, from 0 to 1,
    /// counted in doublings as the eye counts them.
    pub fn level(self) -> f64 {
        self.scale.log2() / Self::MOST.log2()
    }

    /// The picture `factor` times larger, placed so that what was under
    /// `from` is now under `to`. Fingers that pinch around a face keep
    /// that face between them, and fingers that move while they pinch
    /// take the picture along.
    pub fn pinched(self, factor: f64, from: Point, to: Point, view: Size, picture: Size) -> Self {
        let held = self.point_under(from, view, picture);
        let scale = (self.scale * factor).clamp(1.0, Self::MOST);
        let size = scaled(view, picture, scale);
        let focus = Point {
            x: held.x - (to.x - view.width / 2.0) / size.width,
            y: held.y - (to.y - view.height / 2.0) / size.height,
        };
        Self { scale, focus }.kept_in(view, picture)
    }

    /// The picture moved by `by` points.
    pub fn panned(self, by: Point, view: Size, picture: Size) -> Self {
        let middle = Point { x: view.width / 2.0, y: view.height / 2.0 };
        let to = Point { x: middle.x + by.x, y: middle.y + by.y };
        self.pinched(1.0, middle, to, view, picture)
    }

    /// The same zoom with the picture moved as little as it takes for no
    /// edge of it to stand inside the view. Along a side on which it is
    /// no larger than the view it is centred.
    pub fn kept_in(self, view: Size, picture: Size) -> Self {
        let size = scaled(view, picture, self.scale);
        let keep = |focus: f64, picture: f64, view: f64| {
            if picture <= view {
                return 0.5;
            }
            // Half the view, as a fraction of the picture: the focus can
            // come no closer than this to either of its edges.
            let reach = view / 2.0 / picture;
            focus.clamp(reach, 1.0 - reach)
        };
        let focus = Point {
            x: keep(self.focus.x, size.width, view.width),
            y: keep(self.focus.y, size.height, view.height),
        };
        Self { scale: self.scale, focus }
    }

    /// The zoom as mpv takes it: how often the fitted size is doubled, and
    /// how far the picture is moved from the middle to the right and
    /// down, as fractions of its enlarged width and height.
    pub fn for_player(self) -> (f64, (f64, f64)) {
        (self.scale.log2(), (0.5 - self.focus.x, self.focus.y - 0.5))
    }

    /// The point of the picture that is under `place` in the view.
    fn point_under(self, place: Point, view: Size, picture: Size) -> Point {
        let size = scaled(view, picture, self.scale);
        Point {
            x: self.focus.x + (place.x - view.width / 2.0) / size.width,
            y: self.focus.y + (place.y - view.height / 2.0) / size.height,
        }
    }
}

/// The largest size of `picture`'s shape that fits in `view`.
fn fitted(view: Size, picture: Size) -> Size {
    let scale = (view.width / picture.width).min(view.height / picture.height);
    Size { width: picture.width * scale, height: picture.height * scale }
}

fn scaled(view: Size, picture: Size, scale: f64) -> Size {
    let fitted = fitted(view, picture);
    Size { width: fitted.width * scale, height: fitted.height * scale }
}

#[cfg(test)]
mod tests {
    use super::{Point, Size, Zoom};

    const VIEW: Size = Size { width: 1280.0, height: 800.0 };
    /// A 2.39 to 1 film: it fits at 1280 by 536, with bars above and below.
    const FILM: Size = Size { width: 3840.0, height: 1608.0 };

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn what_is_under_the_fingers_stays_under_them() {
        let fingers = Point { x: 900.0, y: 500.0 };
        let before = Zoom::FIT.point_under(fingers, VIEW, FILM);
        let zoomed = Zoom::FIT.pinched(3.0, fingers, fingers, VIEW, FILM);
        assert_eq!(zoomed.scale(), 3.0);
        assert!(close(zoomed.point_under(fingers, VIEW, FILM), before));

        // Fingers that travel while they pinch take the picture along.
        let moved = Point { x: 700.0, y: 450.0 };
        let dragged = zoomed.pinched(1.2, fingers, moved, VIEW, FILM);
        assert!(close(dragged.point_under(moved, VIEW, FILM), before));
    }

    #[test]
    fn the_scale_stops_at_fit_and_at_the_most() {
        let middle = Point { x: 640.0, y: 400.0 };
        assert_eq!(Zoom::FIT.pinched(0.5, middle, middle, VIEW, FILM), Zoom::FIT);
        assert_eq!(Zoom::FIT.pinched(100.0, middle, middle, VIEW, FILM).scale(), 8.0);
    }

    #[test]
    fn no_edge_of_the_picture_comes_into_the_view() {
        // Pinched out at the top right corner, the picture's own corner
        // is all that can be there.
        let corner = Point { x: 1280.0, y: 800.0 };
        let zoomed = Zoom::FIT.pinched(4.0, corner, corner, VIEW, FILM);
        let shown = zoomed.point_under(corner, VIEW, FILM);
        assert!(close(shown, Point { x: 1.0, y: 1.0 }), "{shown:?}");

        // At twice its size the film is 1072 points high in a view of
        // 800: it can be moved up and down, but only so far.
        let far = Point { x: 0.0, y: 5000.0 };
        let panned = Zoom::FIT
            .pinched(2.0, Point { x: 640.0, y: 400.0 }, Point { x: 640.0, y: 400.0 }, VIEW, FILM)
            .panned(far, VIEW, FILM);
        let bottom = panned.point_under(Point { x: 640.0, y: 0.0 }, VIEW, FILM);
        assert!(close(bottom, Point { x: 0.5, y: 0.0 }), "{bottom:?}");
    }

    #[test]
    fn a_picture_shorter_than_the_view_stays_in_the_middle() {
        // At 1.2 times the film is wider than the view and still not as
        // high: it moves sideways only.
        let middle = Point { x: 640.0, y: 400.0 };
        let zoomed = Zoom::FIT.pinched(1.2, middle, middle, VIEW, FILM);
        let panned = zoomed.panned(Point { x: 30.0, y: 200.0 }, VIEW, FILM);
        let (_, (right, down)) = panned.for_player();
        assert!(right > 0.0);
        assert_eq!(down, 0.0);
    }

    #[test]
    fn filling_crops_the_bars_and_no_more() {
        let fill = Zoom::fill(VIEW, FILM);
        assert!((fill.scale() - 800.0 / 536.0).abs() < 1e-9);
        // A picture of the view's own shape already fills it.
        let same = Size { width: 1920.0, height: 1200.0 };
        assert!(Zoom::fill(VIEW, same).is_fit());
    }

    #[test]
    fn the_player_is_told_in_doublings_and_fractions() {
        let corner = Point { x: 0.0, y: 800.0 };
        let (doublings, (right, down)) =
            Zoom::FIT.pinched(2.0, corner, corner, VIEW, FILM).for_player();
        assert_eq!(doublings, 1.0);
        // The top left of the picture is in the view: the picture is as
        // far right and down as it goes.
        assert!(right > 0.0 && down > 0.0, "{right} {down}");
    }
}
