use prns_core::interfaces::wifi_direct::{
    service_instance_platform, Platform, DEVICE_NAME_MARKER, GROUP_SSID_PREFIX,
};

pub const BONJOUR_PTR_QUERY: &[u8] = &[
    0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];

pub const SD_PTR_QUERY_TLV: &[u8] = &[
    0x0d, 0x00, 0x01, 0x01, 0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];

pub fn ptr_response(instance: &str) -> Option<Vec<u8>> {
    let length = u8::try_from(instance.len()).ok()?;
    if length > 63 {
        return None;
    }
    let mut response = Vec::with_capacity(instance.len() + 3);
    response.push(length);
    response.extend_from_slice(instance.as_bytes());
    response.extend_from_slice(&[0xc0, 0x27]);
    Some(response)
}

pub fn instance(tlvs: &[u8]) -> Option<&str> {
    let query = tlvs
        .windows(BONJOUR_PTR_QUERY.len())
        .position(|window| window == BONJOUR_PTR_QUERY)?;
    let length_index = query + BONJOUR_PTR_QUERY.len();
    let length = usize::from(*tlvs.get(length_index)?);
    let label = tlvs.get(length_index + 1..length_index + 1 + length)?;
    core::str::from_utf8(label).ok()
}

pub fn recognized_instance(tlvs: &[u8]) -> Option<&str> {
    let instance = instance(tlvs)?;
    if service_instance_platform(instance).is_some()
        || instance == DEVICE_NAME_MARKER
        || instance.starts_with(GROUP_SSID_PREFIX)
    {
        Some(instance)
    } else {
        None
    }
}

pub fn platform(instance: &str, device_name: &str) -> Platform {
    service_instance_platform(instance).unwrap_or_else(|| {
        if device_name.starts_with(DEVICE_NAME_MARKER) {
            Platform::Supplicant
        } else {
            Platform::Native
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::wifi_direct::{
        NATIVE_SERVICE_INSTANCE, SUPPLICANT_SERVICE_INSTANCE,
    };

    #[test]
    fn a_ptr_response_round_trips_its_instance() {
        let mut response = BONJOUR_PTR_QUERY.to_vec();
        response.extend(ptr_response(SUPPLICANT_SERVICE_INSTANCE).unwrap());
        assert_eq!(instance(&response), Some(SUPPLICANT_SERVICE_INSTANCE));
        assert_eq!(
            recognized_instance(&response),
            Some(SUPPLICANT_SERVICE_INSTANCE)
        );
    }

    #[test]
    fn only_contract_instances_are_recognized() {
        let mut response = BONJOUR_PTR_QUERY.to_vec();
        response.extend(ptr_response("Other").unwrap());
        assert_eq!(recognized_instance(&response), None);

        let mut offer = BONJOUR_PTR_QUERY.to_vec();
        offer.extend(ptr_response("DIRECT-Prns-bench1").unwrap());
        assert_eq!(recognized_instance(&offer), Some("DIRECT-Prns-bench1"));
    }

    #[test]
    fn explicit_platforms_override_mutable_device_names() {
        assert_eq!(
            platform(SUPPLICANT_SERVICE_INSTANCE, "Android"),
            Platform::Supplicant
        );
        assert_eq!(
            platform(NATIVE_SERVICE_INSTANCE, "Prns-device"),
            Platform::Native
        );
        assert_eq!(
            platform(DEVICE_NAME_MARKER, "Prns-old"),
            Platform::Supplicant
        );
        assert_eq!(
            platform(DEVICE_NAME_MARKER, "Android-old"),
            Platform::Native
        );
    }
}
