//! Bounded, read-only overview geometry for dragging group positions in the ordinary window.
use litefences::core::layout::RectDip;

#[derive(Clone, Copy, Debug)]
pub struct RectPx {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RectPx {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Calculates the item area inside a group box, matching the preview renderer.
/// The border is a fixed 2 pixels; header and content spacing scale with DPI.
pub fn item_content(group_box: RectPx, dpi: u32) -> Option<RectPx> {
    if !(48..=768).contains(&dpi) {
        return None;
    }

    let left = i64::from(group_box.left) + 2;
    let top = i64::from(group_box.top) + 2;
    let right = i64::from(group_box.right) - 2;
    let bottom = i64::from(group_box.bottom) - 2;
    let inner_height = bottom - top;
    let scaled = |value: i64| (value * i64::from(dpi) + 48) / 96;
    let header_h = scaled(27).min(inner_height.max(0));
    let content = RectPx {
        left: i32::try_from(left + scaled(5)).ok()?,
        top: i32::try_from(top + header_h + scaled(4)).ok()?,
        right: i32::try_from(right - scaled(5)).ok()?,
        bottom: i32::try_from(bottom - scaled(14)).ok()?,
    };
    (content.right > content.left && content.bottom > content.top).then_some(content)
}

/// Places up to 24 item tiles in `content`, in row-major order.
///
/// At 96 DPI each tile is 42x42 pixels with 4 pixels between tiles. Invalid
/// DPI values, dimensions too small for one full tile, and empty requests
/// return an empty vector.
pub fn item_slots(content: RectPx, dpi: u32, count: usize) -> Vec<RectPx> {
    const MAX_ITEMS: usize = 24;
    if count == 0 || !(48..=768).contains(&dpi) {
        return Vec::new();
    }

    let width = i64::from(content.right) - i64::from(content.left);
    let height = i64::from(content.bottom) - i64::from(content.top);
    if width <= 0 || height <= 0 {
        return Vec::new();
    }

    let scale = i64::from(dpi);
    let tile = (42_i64 * scale + 48) / 96;
    let gap = (4_i64 * scale + 48) / 96;
    if width < tile || height < tile {
        return Vec::new();
    }

    let columns = (width + gap) / (tile + gap);
    let rows = (height + gap) / (tile + gap);
    let capacity = columns.saturating_mul(rows).min(MAX_ITEMS as i64) as usize;
    let slots = count.min(capacity).min(MAX_ITEMS);
    let mut result = Vec::with_capacity(slots);
    for index in 0..slots {
        let column = (index as i64) % columns;
        let row = (index as i64) / columns;
        let left = i64::from(content.left) + column * (tile + gap);
        let top = i64::from(content.top) + row * (tile + gap);
        let right = left + tile;
        let bottom = top + tile;
        // The geometry is contained in the validated i32 content bounds.
        result.push(RectPx {
            left: left as i32,
            top: top as i32,
            right: right as i32,
            bottom: bottom as i32,
        });
    }
    result
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewLayout {
    pub view: RectPx,
    origin_x: i64,
    origin_y: i64,
    scale: f64,
}

impl PreviewLayout {
    pub fn new(view: RectPx, dpi: u32, groups: &[RectDip]) -> Option<Self> {
        let width = view.right.checked_sub(view.left)?;
        let height = view.bottom.checked_sub(view.top)?;
        if width < 100 || height < 100 || !(48..=768).contains(&dpi) {
            return None;
        }
        let mut origin_x = 0_i64;
        let mut origin_y = 0_i64;
        let mut far_x = 320_i64;
        let mut far_y = 240_i64;
        for group in groups {
            origin_x = origin_x.min(i64::from(group.x));
            origin_y = origin_y.min(i64::from(group.y));
            far_x = far_x.max(i64::from(group.x) + i64::from(group.width));
            far_y = far_y.max(i64::from(group.y) + i64::from(group.height));
        }
        let span_x = (far_x - origin_x).max(1);
        let span_y = (far_y - origin_y).max(1);
        let max_scale = f64::from(dpi) / 192.0;
        let scale = max_scale
            .min(f64::from(width - 16) / span_x as f64)
            .min(f64::from(height - 16) / span_y as f64);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        Some(Self {
            view,
            origin_x,
            origin_y,
            scale,
        })
    }

    pub fn group(self, rect: RectDip) -> RectPx {
        let left = self.view.left
            + 8
            + ((i64::from(rect.x) - self.origin_x) as f64 * self.scale).round() as i32;
        let top = self.view.top
            + 8
            + ((i64::from(rect.y) - self.origin_y) as f64 * self.scale).round() as i32;
        RectPx {
            left,
            top,
            right: left + (f64::from(rect.width) * self.scale).round().max(1.0) as i32,
            bottom: top + (f64::from(rect.height) * self.scale).round().max(1.0) as i32,
        }
    }

    pub fn delta_dip(self, dx: i32, dy: i32) -> Option<(i32, i32)> {
        fn one(delta: i32, scale: f64) -> Option<i32> {
            let value = (f64::from(delta) / scale).round();
            (value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX)).then_some(value as i32)
        }
        Some((one(dx, self.scale)?, one(dy, self.scale)?))
    }

    pub fn pixel_delta(self, dx: i32, dy: i32) -> (i32, i32) {
        (
            (f64::from(dx) * self.scale).round() as i32,
            (f64::from(dy) * self.scale).round() as i32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_content_matches_scaled_group_geometry() {
        let group = RectPx {
            left: 100,
            top: 200,
            right: 300,
            bottom: 300,
        };
        for (dpi, expected) in [
            (
                96,
                RectPx {
                    left: 107,
                    top: 233,
                    right: 293,
                    bottom: 284,
                },
            ),
            (
                144,
                RectPx {
                    left: 110,
                    top: 249,
                    right: 290,
                    bottom: 277,
                },
            ),
            (
                192,
                RectPx {
                    left: 112,
                    top: 264,
                    right: 288,
                    bottom: 270,
                },
            ),
        ] {
            let actual = item_content(group, dpi).unwrap();
            assert_eq!(actual.left, expected.left);
            assert_eq!(actual.top, expected.top);
            assert_eq!(actual.right, expected.right);
            assert_eq!(actual.bottom, expected.bottom);
        }
    }

    #[test]
    fn item_content_rejects_invalid_or_unrepresentable_geometry() {
        let extreme = RectPx {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        };
        assert!(item_content(extreme, 0).is_none());
        assert!(item_content(extreme, 769).is_none());
        let content = item_content(extreme, 96).unwrap();
        assert!(content.left > i32::MIN && content.right < i32::MAX);
        assert!(content.top > i32::MIN && content.bottom < i32::MAX);
        assert!(
            item_content(
                RectPx {
                    left: 0,
                    top: 0,
                    right: 30,
                    bottom: 30
                },
                96,
            )
            .is_none()
        );
        assert!(
            item_content(
                RectPx {
                    left: i32::MAX - 10,
                    top: 0,
                    right: i32::MAX,
                    bottom: 100
                },
                96,
            )
            .is_none()
        );
    }

    #[test]
    fn item_slots_scale_and_stay_inside_content() {
        let content = RectPx {
            left: 10,
            top: 20,
            right: 148,
            bottom: 112,
        };
        let slots = item_slots(content, 96, 24);
        assert_eq!(slots.len(), 6);
        assert_eq!(slots[0].right - slots[0].left, 42);
        assert_eq!(slots[0].bottom - slots[0].top, 42);
        assert_eq!(slots[1].left - slots[0].right, 4);
        assert_eq!(slots[3].top - slots[0].top, 46);
        assert!(slots.iter().all(|slot| {
            slot.left >= content.left
                && slot.top >= content.top
                && slot.right <= content.right
                && slot.bottom <= content.bottom
        }));

        let scaled = item_slots(
            RectPx {
                left: 0,
                top: 0,
                right: 92,
                bottom: 92,
            },
            192,
            4,
        );
        assert_eq!(scaled.len(), 1);
        assert_eq!(scaled[0].right, 84);
        assert_eq!(scaled[0].bottom, 84);
    }

    #[test]
    fn item_slots_bound_count_and_reject_invalid_geometry() {
        let large = RectPx {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        };
        assert_eq!(item_slots(large, 96, usize::MAX).len(), 24);
        assert!(item_slots(large, 0, 1).is_empty());
        assert!(item_slots(large, 769, 1).is_empty());
        assert!(
            item_slots(
                RectPx {
                    left: 0,
                    top: 0,
                    right: 41,
                    bottom: 42,
                },
                96,
                1,
            )
            .is_empty()
        );
        assert!(
            item_slots(
                RectPx {
                    left: i32::MAX,
                    top: 0,
                    right: i32::MIN,
                    bottom: 42,
                },
                96,
                1,
            )
            .is_empty()
        );
    }

    #[test]
    fn overview_maps_two_groups_and_drag_delta() {
        let groups = [
            RectDip {
                x: 0,
                y: 0,
                width: 360,
                height: 240,
            },
            RectDip {
                x: 400,
                y: 0,
                width: 360,
                height: 240,
            },
        ];
        let view = RectPx {
            left: 550,
            top: 350,
            right: 1050,
            bottom: 700,
        };
        let layout = PreviewLayout::new(view, 96, &groups).unwrap();
        assert_eq!(layout.group(groups[1]).left, 758);
        assert_eq!(layout.pixel_delta(64, 32), (32, 16));
        assert_eq!(layout.delta_dip(32, 16), Some((64, 32)));
    }

    #[test]
    fn negative_and_wide_layout_fit_without_overflow() {
        let groups = [
            RectDip {
                x: -1_000_000,
                y: -1_000_000,
                width: 16_384,
                height: 16_384,
            },
            RectDip {
                x: 1_000_000,
                y: 1_000_000,
                width: 16_384,
                height: 16_384,
            },
        ];
        let view = RectPx {
            left: 550,
            top: 350,
            right: 1050,
            bottom: 700,
        };
        let layout = PreviewLayout::new(view, 96, &groups).unwrap();
        assert!(layout.group(groups[0]).left >= view.left);
        assert!(layout.group(groups[1]).right <= view.right);
        assert!(layout.delta_dip(1, 1).is_some());
    }
}
