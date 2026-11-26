use super::*;

fn f(x: i32, y: i32, width: u32, height: u32) -> Frame {
    Frame { x, y, width, height, maximized: false }
}

fn area(x: i32, y: i32, width: u32, height: u32) -> Area {
    Area { x, y, width, height }
}

/// A 1920×1080 primary and a 2560×1440 display to its right.
fn two() -> [Area; 2] {
    [area(0, 0, 1920, 1080), area(1920, -200, 2560, 1440)]
}

#[test]
fn a_frame_on_a_display_reopens_where_it_was() {
    assert_eq!(fit(f(100, 80, 1080, 680), &two()), Some(f(100, 80, 1080, 680)));
    assert_eq!(fit(f(2000, 0, 1600, 900), &two()), Some(f(2000, 0, 1600, 900)), "on the second display");
}

#[test]
fn a_frame_off_every_display_comes_back_onto_the_primary() {
    // The second display was unplugged.
    let one = [area(0, 0, 1920, 1080)];
    assert_eq!(fit(f(2600, 300, 1080, 680), &one), Some(f(840, 300, 1080, 680)));
    // Far off up and left.
    assert_eq!(fit(f(-5000, -4000, 1080, 680), &one), Some(f(0, 0, 1080, 680)));
    // A sliver on screen isn't enough: it moves fully in.
    assert_eq!(fit(f(1900, 500, 1080, 680), &one), Some(f(840, 400, 1080, 680)));
}

#[test]
fn a_frame_mostly_on_one_display_moves_onto_it_and_shrinks_to_fit() {
    // Mostly on the second display, hanging below its bottom.
    assert_eq!(fit(f(2500, 1000, 1200, 800), &two()), Some(f(2500, 440, 1200, 800)));
    // Bigger than the display it's on (a smaller one now): no bigger than it, at its corner.
    assert_eq!(fit(f(10, 10, 3000, 2000), &[area(0, 0, 1280, 800)]), Some(f(0, 0, 1280, 800)));
    // The maximized flag comes along.
    let max = Frame { maximized: true, ..f(100, 100, 800, 600) };
    assert_eq!(fit(max, &two()), Some(max));
}

#[test]
fn no_displays_or_an_empty_frame_leave_it_to_the_system() {
    assert_eq!(fit(f(0, 0, 800, 600), &[]), None);
    assert_eq!(fit(f(0, 0, 0, 600), &two()), None);
}

#[test]
fn new_windows_cascade_and_start_over_at_the_edge() {
    let d = two();
    assert_eq!(cascade(f(100, 100, 1080, 680), 28, &d), f(128, 128, 1080, 680));
    // Past the bottom of its display: back at its top-left, one step in.
    assert_eq!(cascade(f(400, 390, 1080, 680), 28, &d), f(28, 28, 1080, 680));
    // On the second display it stays there.
    assert_eq!(cascade(f(4400, 0, 1080, 680), 56, &d), f(1976, -144, 1080, 680));
    // A maximized opener's copy isn't maximized; as big as the display, it can't step.
    let max = Frame { maximized: true, ..f(0, 0, 1920, 1080) };
    assert_eq!(cascade(max, 28, &d), f(0, 0, 1920, 1080));
    // No displays known: just the step.
    assert_eq!(cascade(f(10, 10, 800, 600), 28, &[]), f(38, 38, 800, 600));
}
