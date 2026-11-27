use super::*;

#[test]
fn the_app_icon_is_a_256_px_png() {
    assert!(APP_ICON.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
    let icon = ImageCache::new(Arc::new(|| {})).load(APP_ICON);
    assert_eq!(icon.wait().map_err(|e| e.0.clone()), Ok([256, 256]));
}

#[test]
fn the_app_builds_in_the_brand_kits_own_files() {
    let files: [(&str, &[u8], &[u8]); 5] = [
        ("dromon-256.png", APP_ICON, include_bytes!("../../../../assets/brand/png/app-icon/dromon-256.png")),
        ("dromon-mark.svg", Mark::Full.svg(true), include_bytes!("../../../../assets/brand/svg/dromon-mark.svg")),
        (
            "dromon-mark-on-light.svg",
            Mark::Full.svg(false),
            include_bytes!("../../../../assets/brand/svg/dromon-mark-on-light.svg"),
        ),
        (
            "dromon-mark-simple.svg",
            Mark::Simple.svg(true),
            include_bytes!("../../../../assets/brand/svg/dromon-mark-simple.svg"),
        ),
        (
            "dromon-mark-simple-on-light.svg",
            Mark::Simple.svg(false),
            include_bytes!("../../../../assets/brand/svg/dromon-mark-simple-on-light.svg"),
        ),
    ];
    for (name, app, kit) in files {
        assert!(app == kit, "crates/dromon-app/assets/brand/{name} isn't the kit's: run `cargo brand`");
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn rune_takes_the_app_icon() {
    assert_eq!(rune::set_app_icon_choice(rune::AppIcon::Image(APP_ICON.to_vec())), Ok(()));
}

#[test]
fn every_mark_rasterizes_at_its_size_for_both_themes_and_scales() {
    for mark in [Mark::Simple, Mark::Full] {
        let (w, h) = mark.size();
        for dark in [true, false] {
            for scale in [1.0, 2.0] {
                let px = [(w * scale) as u32, (h * scale) as u32];
                let image = image(mark, dark, scale);
                assert_eq!(image.wait().map_err(|e| e.0.clone()), Ok(px), "{mark:?}, dark {dark}, {scale}x");
            }
        }
    }
}

#[test]
fn marks_are_mint_on_dark_and_deep_mint_on_light() {
    for mark in [Mark::Simple, Mark::Full] {
        let dark = String::from_utf8_lossy(mark.svg(true));
        let light = String::from_utf8_lossy(mark.svg(false));
        assert!(dark.contains("#3DDC97") && !dark.contains("#0E9F6E"), "{mark:?} on dark");
        assert!(light.contains("#0E9F6E") && !light.contains("#3DDC97"), "{mark:?} on light");
    }
}

#[test]
fn a_mark_is_drawn_once_per_theme_and_scale() {
    let first = image(Mark::Simple, true, 2.0);
    assert_eq!(image(Mark::Simple, true, 2.0).id(), first.id());
    assert_ne!(image(Mark::Simple, false, 2.0).id(), first.id());
    assert_ne!(image(Mark::Simple, true, 1.0).id(), first.id());
}
