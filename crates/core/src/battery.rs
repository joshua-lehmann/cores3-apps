//! Decode the AXP2101 battery presence, fuel gauge and charge direction.

/// A valid battery reading, independent of whether USB power is connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// Fuel-gauge estimate from zero to one hundred.
    pub percent: u8,
    /// True only while the PMIC reports current charging the battery.
    pub charging: bool,
}

/// Decode status registers 0x00 and 0x01 and fuel-gauge register 0xa4.
pub fn decode(presence: u8, state: u8, percent: u8) -> Option<Reading> {
    (presence & 8 != 0 && percent <= 100).then_some(Reading {
        percent,
        charging: state >> 5 == 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_charging_from_usb_connected_and_full() {
        assert!(decode(0x28, 0x20, 30).unwrap().charging);
        assert!(!decode(0x28, 0x00, 100).unwrap().charging);
        assert!(!decode(0x08, 0x40, 10).unwrap().charging);
    }

    #[test]
    fn rejects_absent_and_invalid_batteries() {
        assert_eq!(decode(0x20, 0x20, 50), None);
        assert_eq!(decode(8, 0x20, 255), None);
        assert_eq!(decode(8, 0, 0).unwrap().percent, 0);
    }
}
