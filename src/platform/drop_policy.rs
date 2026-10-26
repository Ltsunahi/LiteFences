//! Pure policy for choosing a safe shell drop effect.
//!
//! These values are the `DROPEFFECT_*` flags from the Windows Shell API. Keeping
//! this decision independent of COM makes the supported behavior easy to test.

/// `DROPEFFECT_NONE`: the target does not accept the drop.
pub const DROPEFFECT_NONE: u32 = 0;
/// `DROPEFFECT_COPY`: accept the drop by copying/adding references.
pub const DROPEFFECT_COPY: u32 = 1;
/// `DROPEFFECT_MOVE`: deliberately never selected by this policy.
pub const DROPEFFECT_MOVE: u32 = 2;
/// `DROPEFFECT_LINK`: accept the drop by creating references/shortcuts.
pub const DROPEFFECT_LINK: u32 = 4;

/// Choose a drop effect without ever allowing the source to move its items.
///
/// LINK is preferred only when paths are supported and the source offered it.
/// COPY is the fallback when offered; unsupported or absent effects yield NONE.
pub fn choose_drop_effect(allowed: u32, has_supported_paths: bool) -> u32 {
    if !has_supported_paths {
        return DROPEFFECT_NONE;
    }
    if allowed & DROPEFFECT_LINK != 0 {
        DROPEFFECT_LINK
    } else if allowed & DROPEFFECT_COPY != 0 {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_safe_offered_effects() {
        let cases = [
            ("link with paths", DROPEFFECT_LINK, true, DROPEFFECT_LINK),
            (
                "link without paths",
                DROPEFFECT_LINK,
                false,
                DROPEFFECT_NONE,
            ),
            ("copy", DROPEFFECT_COPY, true, DROPEFFECT_COPY),
            ("move only", DROPEFFECT_MOVE, true, DROPEFFECT_NONE),
            (
                "link and copy prefers link",
                DROPEFFECT_LINK | DROPEFFECT_COPY,
                true,
                DROPEFFECT_LINK,
            ),
            (
                "link and copy without paths",
                DROPEFFECT_LINK | DROPEFFECT_COPY,
                false,
                DROPEFFECT_NONE,
            ),
            (
                "copy and move prefers copy",
                DROPEFFECT_COPY | DROPEFFECT_MOVE,
                true,
                DROPEFFECT_COPY,
            ),
            (
                "all known effects",
                DROPEFFECT_LINK | DROPEFFECT_COPY | DROPEFFECT_MOVE,
                true,
                DROPEFFECT_LINK,
            ),
            ("invalid bit", 0x8000_0000, true, DROPEFFECT_NONE),
            (
                "invalid bit and move",
                0x8000_0000 | DROPEFFECT_MOVE,
                true,
                DROPEFFECT_NONE,
            ),
            (
                "no supported paths",
                DROPEFFECT_LINK,
                false,
                DROPEFFECT_NONE,
            ),
            ("no offered effects", DROPEFFECT_NONE, true, DROPEFFECT_NONE),
        ];

        for (name, allowed, has_paths, expected) in cases {
            let chosen = choose_drop_effect(allowed, has_paths);
            assert_eq!(chosen, expected, "case: {name}");
            assert_eq!(chosen & !allowed, 0, "result not in allowed set: {name}");
            assert_eq!(chosen & DROPEFFECT_MOVE, 0, "MOVE selected: {name}");
        }
    }
}
