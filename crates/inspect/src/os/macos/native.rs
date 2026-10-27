//! IOKit ownership and typed dictionary access stay inside this native boundary.
use super::battery::{Battery, Kind, Power};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::CFString,
};

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, source: CFTypeRef) -> CFDictionaryRef;
}

type Dictionary = CFDictionary<CFString, CFType>;

pub(super) fn low_power_mode() -> bool {
    objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled()
}

fn number(dictionary: &Dictionary, key: &str) -> Option<i64> {
    dictionary
        .find(CFString::new(key))?
        .downcast::<CFNumber>()?
        .to_i64()
}

fn boolean(dictionary: &Dictionary, key: &str) -> Option<bool> {
    dictionary
        .find(CFString::new(key))?
        .downcast::<CFBoolean>()
        .map(bool::from)
}

fn category<T: Copy>(dictionary: &Dictionary, key: &str, values: &[(&str, T)]) -> Option<T> {
    let value = dictionary
        .find(CFString::new(key))?
        .downcast::<CFString>()?;
    values
        .iter()
        .find(|(expected, _)| value == CFString::new(expected))
        .map(|(_, category)| *category)
}

fn battery(dictionary: &Dictionary) -> Battery {
    Battery {
        local_id: number(dictionary, "Power Source ID"),
        present: boolean(dictionary, "Is Present"),
        kind: category(
            dictionary,
            "Type",
            &[("InternalBattery", Kind::System), ("UPS", Kind::Ups)],
        ),
        current: number(dictionary, "Current Capacity"),
        maximum: number(dictionary, "Max Capacity"),
        charging: boolean(dictionary, "Is Charging"),
        power: category(
            dictionary,
            "Power Source State",
            &[
                ("AC Power", Power::Ac),
                ("Battery Power", Power::Battery),
                ("Off Line", Power::Offline),
            ],
        ),
        voltage_mv: number(dictionary, "Voltage"),
        temperature_c: number(dictionary, "Temperature"),
        empty_minutes: number(dictionary, "Time to Empty"),
        full_minutes: number(dictionary, "Time to Full Charge"),
    }
}

pub(super) fn batteries() -> Result<Vec<Option<Battery>>, ()> {
    // SAFETY: read-only IOKit API without arguments; the returned Copy object is owned.
    let info = unsafe { IOPSCopyPowerSourcesInfo() };
    if info.is_null() {
        return Err(());
    }
    // SAFETY: non-null retained CFType returned by IOPSCopyPowerSourcesInfo.
    let info = unsafe { CFType::wrap_under_create_rule(info) };
    // SAFETY: info is the opaque live snapshot from the preceding IOKit call.
    let sources = unsafe { IOPSCopyPowerSourcesList(info.as_CFTypeRef()) };
    if sources.is_null() {
        return Err(());
    }
    // SAFETY: API documents a retained CFArray of CFType handles, owned until drop.
    let sources: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(sources) };
    let mut result = Vec::new();
    for source in sources.iter() {
        // SAFETY: source belongs to this live snapshot; this Get result is borrowed.
        let dictionary =
            unsafe { IOPSGetPowerSourceDescription(info.as_CFTypeRef(), source.as_CFTypeRef()) };
        if dictionary.is_null() {
            result.push(None);
            continue;
        }
        // SAFETY: non-null dictionary with CFString keys and CFType values per IOPSKeys.h.
        // Get wrapping retains it for this scope; neither the blob nor its children escape.
        let dictionary: Dictionary = unsafe { CFDictionary::wrap_under_get_rule(dictionary) };
        result.push(Some(battery(&dictionary)));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_values_are_type_checked_and_unknown_categories_stay_unknown() {
        let dictionary = Dictionary::from_CFType_pairs(&[
            (
                CFString::new("Current Capacity"),
                CFString::new("secret").as_CFType(),
            ),
            (
                CFString::new("Max Capacity"),
                CFNumber::from(100_i64).as_CFType(),
            ),
            (
                CFString::new("Power Source State"),
                CFString::new("private device").as_CFType(),
            ),
            (
                CFString::new("Is Present"),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::new("Is Charging"),
                CFNumber::from(1_i64).as_CFType(),
            ),
        ]);
        let read = battery(&dictionary);
        assert_eq!(read.current, None);
        assert_eq!(read.maximum, Some(100));
        assert_eq!(read.power, None);
        assert_eq!(read.present, Some(true));
        assert_eq!(read.charging, None);
    }
}
