//! The bundled faces (`assets/fonts/`, both SIL OFL 1.1, licences beside them): Inter 4.1
//! variable from rsms/inter's release and Geist Mono 1.9.2 variable from vercel/geist-font's.

/// For `AppSpec::fonts`; replaces Rune's built-in Inter with ours.
pub const FONTS: &[&[u8]] = &[
    include_bytes!("../../../../assets/fonts/InterVariable.ttf"),
    include_bytes!("../../../../assets/fonts/GeistMonoVariable.ttf"),
];

/// `AppSpec::families`: (sans, mono). Upstream's variable Inter names its family
/// "Inter Variable" (the static cuts are "Inter").
pub const FAMILIES: (&str, &str) = ("Inter Variable", "Geist Mono");
