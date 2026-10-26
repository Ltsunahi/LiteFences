//! Shared layout geometry expressed in device-independent pixels (DIP).

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RectDip {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    PositionOutOfRange,
    SizeOutOfRange,
    InvalidDpi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl RectDip {
    pub fn validate(&self) -> Result<(), LayoutError> {
        if self.x < -1_000_000 || self.x > 1_000_000 || self.y < -1_000_000 || self.y > 1_000_000 {
            return Err(LayoutError::PositionOutOfRange);
        }

        if !(240..=16_384).contains(&self.width) || !(120..=16_384).contains(&self.height) {
            return Err(LayoutError::SizeOutOfRange);
        }

        Ok(())
    }

    pub fn to_pixels(&self, dpi: u32) -> Result<PixelRect, LayoutError> {
        self.validate()?;
        if !(48..=768).contains(&dpi) {
            return Err(LayoutError::InvalidDpi);
        }

        let dpi = i64::from(dpi);
        Ok(PixelRect {
            x: round_half_away_from_zero(i64::from(self.x) * dpi, 96) as i32,
            y: round_half_away_from_zero(i64::from(self.y) * dpi, 96) as i32,
            width: round_half_away_from_zero(i64::from(self.width) * dpi, 96) as u32,
            height: round_half_away_from_zero(i64::from(self.height) * dpi, 96) as u32,
        })
    }
}

fn round_half_away_from_zero(numerator: i64, denominator: i64) -> i64 {
    let sign = if numerator < 0 { -1 } else { 1 };
    sign * ((numerator.abs() + denominator / 2) / denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> RectDip {
        RectDip {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn validates_position_boundaries_and_negative_coordinates() {
        assert_eq!(rect(-1_000_000, 1_000_000, 240, 120).validate(), Ok(()));
        assert_eq!(
            rect(-1_000_001, 0, 240, 120).validate(),
            Err(LayoutError::PositionOutOfRange)
        );
        assert_eq!(
            rect(0, 1_000_001, 240, 120).validate(),
            Err(LayoutError::PositionOutOfRange)
        );
        assert_eq!(
            rect(-240, -120, 240, 120).to_pixels(96),
            Ok(PixelRect {
                x: -240,
                y: -120,
                width: 240,
                height: 120
            })
        );
    }

    #[test]
    fn validates_size_boundaries() {
        assert_eq!(rect(0, 0, 240, 120).validate(), Ok(()));
        assert_eq!(rect(0, 0, 16_384, 16_384).validate(), Ok(()));
        assert_eq!(
            rect(0, 0, 239, 120).validate(),
            Err(LayoutError::SizeOutOfRange)
        );
        assert_eq!(
            rect(0, 0, 240, 119).validate(),
            Err(LayoutError::SizeOutOfRange)
        );
        assert_eq!(
            rect(0, 0, 16_385, 120).validate(),
            Err(LayoutError::SizeOutOfRange)
        );
        assert_eq!(
            rect(0, 0, 240, 16_385).validate(),
            Err(LayoutError::SizeOutOfRange)
        );
    }

    #[test]
    fn rejects_zero_and_out_of_range_dpi() {
        let value = rect(0, 0, 240, 120);
        assert_eq!(value.to_pixels(0), Err(LayoutError::InvalidDpi));
        assert_eq!(value.to_pixels(47), Err(LayoutError::InvalidDpi));
        assert_eq!(value.to_pixels(769), Err(LayoutError::InvalidDpi));
        assert_eq!(value.to_pixels(u32::MAX), Err(LayoutError::InvalidDpi));
    }

    #[test]
    fn converts_common_dpi_values() {
        let value = rect(-100, -50, 240, 120);
        assert_eq!(
            value.to_pixels(48),
            Ok(PixelRect {
                x: -50,
                y: -25,
                width: 120,
                height: 60
            })
        );
        assert_eq!(
            value.to_pixels(96),
            Ok(PixelRect {
                x: -100,
                y: -50,
                width: 240,
                height: 120
            })
        );
        assert_eq!(
            value.to_pixels(120),
            Ok(PixelRect {
                x: -125,
                y: -63,
                width: 300,
                height: 150
            })
        );
        assert_eq!(
            value.to_pixels(144),
            Ok(PixelRect {
                x: -150,
                y: -75,
                width: 360,
                height: 180
            })
        );
        assert_eq!(
            value.to_pixels(192),
            Ok(PixelRect {
                x: -200,
                y: -100,
                width: 480,
                height: 240
            })
        );
        assert_eq!(
            value.to_pixels(768),
            Ok(PixelRect {
                x: -800,
                y: -400,
                width: 1_920,
                height: 960
            })
        );
    }

    #[test]
    fn rounds_coordinates_half_away_from_zero() {
        assert_eq!(rect(-1, -1, 240, 120).to_pixels(144).unwrap().x, -2);
        assert_eq!(rect(1, 1, 240, 120).to_pixels(144).unwrap().x, 2);
        assert_eq!(rect(-2, -2, 240, 120).to_pixels(120).unwrap().x, -3);
        assert_eq!(rect(2, 2, 240, 120).to_pixels(120).unwrap().x, 3);
    }

    #[test]
    fn rejects_overflow_prone_input_before_conversion() {
        assert_eq!(
            rect(i32::MIN, 0, 240, 120).validate(),
            Err(LayoutError::PositionOutOfRange)
        );
        assert_eq!(
            rect(i32::MAX, 0, 240, 120).validate(),
            Err(LayoutError::PositionOutOfRange)
        );
        assert_eq!(
            rect(0, 0, u32::MAX, 120).validate(),
            Err(LayoutError::SizeOutOfRange)
        );
        assert_eq!(
            rect(0, 0, 240, u32::MAX).validate(),
            Err(LayoutError::SizeOutOfRange)
        );
        assert_eq!(
            rect(i32::MAX, 0, u32::MAX, u32::MAX).to_pixels(768),
            Err(LayoutError::PositionOutOfRange)
        );
    }
}
