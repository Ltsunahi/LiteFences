//! Pure pointer-drag activation and destination policy for items.
//!
//! This module only tracks drag intent. It performs no file I/O, persistence,
//! or drawing.

use litefences::core::model::{GroupId, ItemId};

/// Tracks whether an item pointer gesture has crossed the drag threshold.
#[derive(Clone, Copy, Debug)]
pub struct ItemDrag {
    pub source: GroupId,
    pub item: ItemId,
    start: (i32, i32),
    pub active: bool,
}

impl ItemDrag {
    pub fn new(source: GroupId, item: ItemId, x: i32, y: i32) -> Self {
        Self {
            source,
            item,
            start: (x, y),
            active: false,
        }
    }

    /// Updates the pointer and returns true only when this update first
    /// activates the drag. `dpi` is expected to be in the normal 96..=768 range.
    pub fn update(&mut self, x: i32, y: i32, dpi: u32) -> bool {
        if self.active {
            return false;
        }

        let threshold = (4_u64 * u64::from(dpi)).div_ceil(96).max(4) as i64;
        let dx = (i64::from(x) - i64::from(self.start.0)).abs();
        let dy = (i64::from(y) - i64::from(self.start.1)).abs();
        if dx >= threshold || dy >= threshold {
            self.active = true;
            true
        } else {
            false
        }
    }

    pub fn destination(self, hover: Option<GroupId>) -> Option<GroupId> {
        if self.active {
            hover.filter(|group| *group != self.source)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ItemDrag;
    use litefences::core::model::{GroupId, ItemId};

    fn group(id: u64) -> GroupId {
        GroupId::new(id).unwrap()
    }

    fn item(id: u64) -> ItemId {
        ItemId::new(id).unwrap()
    }

    #[test]
    fn click_and_threshold_boundary() {
        let mut drag = ItemDrag::new(group(1), item(1), 10, 20);
        assert!(!drag.update(13, 20, 96));
        assert!(!drag.active);
        assert!(drag.update(14, 20, 96));
        assert!(drag.active);
    }

    #[test]
    fn axis_threshold_triggers_but_diagonal_subthreshold_does_not() {
        let mut diagonal = ItemDrag::new(group(1), item(1), 0, 0);
        assert!(!diagonal.update(3, 3, 96));

        let mut axis = ItemDrag::new(group(1), item(1), 0, 0);
        assert!(axis.update(0, -4, 96));
    }

    #[test]
    fn scales_threshold_at_150_and_200_percent_dpi() {
        let mut at_150 = ItemDrag::new(group(1), item(1), 0, 0);
        assert!(!at_150.update(5, 0, 144));
        assert!(at_150.update(6, 0, 144));

        let mut at_200 = ItemDrag::new(group(1), item(1), 0, 0);
        assert!(!at_200.update(7, 0, 192));
        assert!(at_200.update(8, 0, 192));
    }

    #[test]
    fn extreme_coordinates_do_not_overflow() {
        let mut drag = ItemDrag::new(group(1), item(1), i32::MIN, i32::MAX);
        assert!(drag.update(i32::MAX, i32::MIN, 96));
    }

    #[test]
    fn activation_is_latched_and_same_group_is_not_a_destination() {
        let source = group(1);
        let other = group(2);
        let mut drag = ItemDrag::new(source, item(1), 0, 0);
        assert_eq!(drag.destination(Some(other)), None);
        assert!(drag.update(4, 0, 96));
        assert_eq!(drag.destination(Some(source)), None);
        assert_eq!(drag.destination(None), None);
        assert_eq!(drag.destination(Some(other)), Some(other));
        assert!(!drag.update(0, 0, 96));
        assert!(drag.active);
    }
}
