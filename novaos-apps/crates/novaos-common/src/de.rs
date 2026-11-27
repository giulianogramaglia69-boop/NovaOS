//! I cinque desktop NovaOS e le loro immagini GHCR.
//!
//! Un'immagine per DE, cambio via `bootc switch <ref>` (fallback
//! `rpm-ostree rebase ostree-unverified-registry:<ref>`).

/// Vendor GHCR (owner GitHub).
pub const IMAGE_VENDOR: &str = "giulianogramaglia69-boop";
/// Tag usato per tutte le immagini.
pub const IMAGE_TAG: &str = "45";

/// (id desktop, nome immagine) — l'ordine è quello del README.
pub const DES: [(&str, &str); 5] = [
    ("gnome", "novaos-gnome"),
    ("cosmic", "novaos-cosmic"),
    ("hyprland", "novaos-hyprland"),
    ("mangowc", "novaos-mangowc"),
    ("niri", "novaos-niri"),
];

/// Etichetta leggibile del desktop.
pub fn label(de: &str) -> &str {
    match de {
        "gnome" => "GNOME",
        "cosmic" => "COSMIC",
        "hyprland" => "Hyprland",
        "mangowc" => "MangoWC",
        "niri" => "Niri",
        other => other,
    }
}

/// `true` se `de` è uno dei cinque desktop NovaOS.
pub fn is_known_de(de: &str) -> bool {
    DES.iter().any(|(id, _)| *id == de)
}

/// Nome immagine per un desktop (`niri` → `novaos-niri`).
pub fn image_name(de: &str) -> Option<&'static str> {
    DES.iter().find(|(id, _)| *id == de).map(|(_, name)| *name)
}

/// Ref completa GHCR (`ghcr.io/giulianogramaglia69-boop/novaos-niri:45`).
pub fn image_ref(de: &str) -> Option<String> {
    image_name(de).map(|name| format!("ghcr.io/{IMAGE_VENDOR}/{name}:{IMAGE_TAG}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_are_correct() {
        assert_eq!(
            image_ref("niri").as_deref(),
            Some("ghcr.io/giulianogramaglia69-boop/novaos-niri:45")
        );
        assert_eq!(
            image_ref("cosmic").as_deref(),
            Some("ghcr.io/giulianogramaglia69-boop/novaos-cosmic:45")
        );
        assert_eq!(image_ref("cinnamon"), None);
        assert!(is_known_de("hyprland"));
        assert!(!is_known_de("kde"));
        assert_eq!(label("mangowc"), "MangoWC");
    }
}
