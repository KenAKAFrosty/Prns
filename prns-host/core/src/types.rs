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

fixed_bytes!(DestinationHash, 16);
fixed_bytes!(IdentityHash, 16);
fixed_bytes!(InterfaceId, 8);
fixed_bytes!(LinkId, 16);
fixed_bytes!(RequestId, 16);
fixed_bytes!(RequestPathHash, 16);
fixed_bytes!(ResourceHash, 32);

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
