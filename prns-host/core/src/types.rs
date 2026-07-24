use crate::{
    DESTINATION_HASH_LENGTH, IDENTITY_HASH_LENGTH, INTERFACE_ID_LENGTH, LINK_ID_LENGTH,
    REQUEST_ID_LENGTH, REQUEST_PATH_HASH_LENGTH, RESOURCE_HASH_LENGTH,
};

macro_rules! fixed_bytes {
    ($name:ident, $length:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; $length]);

        impl $name {
            pub const LENGTH: usize = $length;

            #[must_use]
            pub const fn new(bytes: [u8; $length]) -> Self {
                Self(bytes)
            }

            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $length] {
                &self.0
            }

            #[must_use]
            pub const fn into_bytes(self) -> [u8; $length] {
                self.0
            }
        }
    };
}

fixed_bytes!(DestinationHash, DESTINATION_HASH_LENGTH);
fixed_bytes!(IdentityHash, IDENTITY_HASH_LENGTH);
fixed_bytes!(InterfaceId, INTERFACE_ID_LENGTH);
fixed_bytes!(LinkId, LINK_ID_LENGTH);
fixed_bytes!(RequestId, REQUEST_ID_LENGTH);
fixed_bytes!(RequestPathHash, REQUEST_PATH_HASH_LENGTH);
fixed_bytes!(ResourceHash, RESOURCE_HASH_LENGTH);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(u64);

impl CommandId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}
