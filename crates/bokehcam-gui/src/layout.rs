//! How the window divides its space, as plain arithmetic on sizes.

/// How the preview and the settings share the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    /// Preview on the left, settings in a panel on the right.
    SideBySide,
    /// Preview on top, settings below.
    Stacked,
}

/// Width of the settings panel beside the preview, until the divider is
/// dragged.
pub const SETTINGS_WIDTH: f32 = 460.0;
/// Share of the window height the settings take below the preview, until the
/// divider is dragged.
pub const STACKED_SETTINGS_SHARE: f32 = 0.4;

impl Arrangement {
    /// The arrangement that shows a picture of these proportions larger in a
    /// window of this size, with the settings at their initial size.
    pub fn for_window(window: (f32, f32), picture: (u32, u32)) -> Self {
        let beside = fit(picture, (window.0 - SETTINGS_WIDTH, window.1));
        let stacked = fit(picture, (window.0, window.1 * (1.0 - STACKED_SETTINGS_SHARE)));
        if beside.0 >= stacked.0 { Self::SideBySide } else { Self::Stacked }
    }
}

/// The largest size with the proportions of `picture` that fits in
/// `available`. Empty when either has no area.
pub fn fit(picture: (u32, u32), available: (f32, f32)) -> (f32, f32) {
    let (width, height) = (picture.0 as f32, picture.1 as f32);
    if width <= 0.0 || height <= 0.0 || available.0 <= 0.0 || available.1 <= 0.0 {
        return (0.0, 0.0);
    }
    let scale = (available.0 / width).min(available.1 / height);
    (width * scale, height * scale)
}

/// The size to ask the engine for so the preview is sharp when it is shown
/// `shown_width` physical pixels wide: the smallest quarter step of the
/// output that covers it, and never more than the output itself.
///
/// Steps keep the request stable while the divider is dragged, so the engine
/// does not rebuild its preview for every pixel of movement.
pub fn preview_request(output: (u32, u32), shown_width: f32) -> (u32, u32) {
    const STEPS: u32 = 4;
    let step = (1..STEPS).find(|step| (output.0 * step / STEPS) as f32 >= shown_width).unwrap_or(STEPS);
    (output.0 * step / STEPS, output.1 * step / STEPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDESCREEN: (u32, u32) = (1920, 1080);

    #[test]
    fn wide_windows_put_the_settings_beside_the_preview() {
        assert_eq!(Arrangement::for_window((1920.0, 1080.0), WIDESCREEN), Arrangement::SideBySide);
        assert_eq!(Arrangement::for_window((3440.0, 1440.0), WIDESCREEN), Arrangement::SideBySide);
    }

    #[test]
    fn narrow_or_tall_windows_stack() {
        assert_eq!(Arrangement::for_window((600.0, 800.0), WIDESCREEN), Arrangement::Stacked);
        assert_eq!(Arrangement::for_window((1000.0, 1400.0), WIDESCREEN), Arrangement::Stacked);
    }

    #[test]
    fn a_nearly_square_window_stacks_because_the_picture_is_larger_that_way() {
        assert_eq!(Arrangement::for_window((1716.0, 1410.0), WIDESCREEN), Arrangement::Stacked);
    }

    #[test]
    fn a_tall_picture_prefers_the_settings_beside_it() {
        assert_eq!(Arrangement::for_window((1000.0, 1000.0), (1080, 1920)), Arrangement::SideBySide);
    }

    #[test]
    fn a_window_narrower_than_the_settings_panel_stacks() {
        assert_eq!(Arrangement::for_window((400.0, 300.0), WIDESCREEN), Arrangement::Stacked);
    }

    #[test]
    fn fit_is_limited_by_the_tighter_side() {
        assert_eq!(fit((1920, 1080), (1600.0, 2000.0)), (1600.0, 900.0));
        assert_eq!(fit((1920, 1080), (3000.0, 540.0)), (960.0, 540.0));
    }

    #[test]
    fn fit_keeps_other_proportions() {
        assert_eq!(fit((640, 480), (1000.0, 300.0)), (400.0, 300.0));
    }

    #[test]
    fn fit_scales_up_as_well_as_down() {
        assert_eq!(fit((320, 180), (1280.0, 1000.0)), (1280.0, 720.0));
    }

    #[test]
    fn fit_with_no_room_or_no_picture_is_empty() {
        assert_eq!(fit((1920, 1080), (0.0, 500.0)), (0.0, 0.0));
        assert_eq!(fit((1920, 1080), (500.0, -4.0)), (0.0, 0.0));
        assert_eq!(fit((0, 1080), (500.0, 500.0)), (0.0, 0.0));
    }

    #[test]
    fn preview_request_is_the_smallest_step_that_covers_what_is_shown() {
        assert_eq!(preview_request(WIDESCREEN, 300.0), (480, 270));
        assert_eq!(preview_request(WIDESCREEN, 480.0), (480, 270));
        assert_eq!(preview_request(WIDESCREEN, 481.0), (960, 540));
        assert_eq!(preview_request(WIDESCREEN, 1392.0), (1440, 810));
        assert_eq!(preview_request(WIDESCREEN, 1441.0), (1920, 1080));
    }

    #[test]
    fn preview_request_never_exceeds_the_output() {
        assert_eq!(preview_request(WIDESCREEN, 5000.0), (1920, 1080));
        assert_eq!(preview_request((640, 480), 2000.0), (640, 480));
    }

    #[test]
    fn preview_request_before_anything_is_shown_is_the_smallest_step() {
        assert_eq!(preview_request(WIDESCREEN, 0.0), (480, 270));
    }
}
