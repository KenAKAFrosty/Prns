use super::DeviceRegistry;

impl DeviceRegistry {
    pub fn exhaust_enrollment_identifiers_for_test(&mut self) {
        self.next_enrollment = None;
    }
}
