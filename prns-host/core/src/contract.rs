use alloc::string::String;

use crate::{HOST_SCHEMA_ABI, HOST_SCHEMA_PRODUCT_VERSION};

pub const HOST_CONTRACT_ABI: u32 = HOST_SCHEMA_ABI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostContract {
    pub abi: u32,
    pub product_version: &'static str,
}

pub const HOST_CONTRACT: HostContract = HostContract {
    abi: HOST_CONTRACT_ABI,
    product_version: HOST_SCHEMA_PRODUCT_VERSION,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostContractMismatch {
    Abi {
        required: u32,
        actual: u32,
    },
    ProductVersion {
        required: String,
        actual: &'static str,
    },
}

pub fn verify_host_contract(
    required_abi: u32,
    required_product_version: &str,
) -> Result<HostContract, HostContractMismatch> {
    if required_abi != HOST_CONTRACT.abi {
        return Err(HostContractMismatch::Abi {
            required: required_abi,
            actual: HOST_CONTRACT.abi,
        });
    }
    if required_product_version != HOST_CONTRACT.product_version {
        return Err(HostContractMismatch::ProductVersion {
            required: required_product_version.into(),
            actual: HOST_CONTRACT.product_version,
        });
    }
    Ok(HOST_CONTRACT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_and_schema_versions_are_one_release() {
        assert_eq!(HOST_CONTRACT.product_version, env!("CARGO_PKG_VERSION"));
    }
}
