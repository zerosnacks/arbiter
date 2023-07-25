#![warn(missing_docs)]
//! Module for utility functionality.
use std::{
    error::Error,
    fmt::{Display, Formatter, Result as FmtResult},
};

use bytes::Bytes;
use ethers::{prelude::Address, types::H256};
use revm::primitives::{ExecutionResult, Output, B160, B256};

#[derive(Debug)]
// We should use anyhow / thisError instead
/// Error type for the simulation manager.
/// # Fields
/// * `message` - Error message.
/// * `output` - Byte output of the error.
pub struct UnpackError {
    /// Error message.
    pub message: String,
    /// Byte output of the error.
    pub output: Option<Bytes>,
}

impl Error for UnpackError {}

impl Display for UnpackError {
    /// Display the error message.
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        write!(f, "{}", self.message)
    }
}

// Certainly will go away with alloy-types
/// Recast a B160 into an Address type
/// # Arguments
/// * `address` - B160 to recast. (B160)
/// # Returns
/// * `Address` - Recasted Address.
pub fn recast_address(address: B160) -> Address {
    let temp: [u8; 20] = address.as_bytes().try_into().unwrap();
    Address::from(temp)
}

pub fn recast_b256(input: B256) -> H256 {
    let temp: [u8; 32] = input.as_bytes().try_into().unwrap();
    H256::from(temp)
}
// TODO: Can maybe get rid of this with middleware
/// Takes an `ExecutionResult` and returns the raw bytes of the output that can then be decoded.
/// # Arguments
/// * `execution_result` - The `ExecutionResult` that we want to unpack.
/// # Returns
/// * `Ok(Bytes)` - The raw bytes of the output.
pub fn unpack_execution(execution_result: ExecutionResult) -> Result<Bytes, UnpackError> {
    match execution_result {
        ExecutionResult::Success { output, .. } => match output {
            Output::Call(value) => Ok(value),
            Output::Create(value, _address) => Ok(value),
        },
        ExecutionResult::Halt { reason, gas_used } => Err(UnpackError {
            message: format!(
                "This call halted for {:#?} and used {} gas.",
                reason, gas_used
            ),
            output: None,
        }),
        ExecutionResult::Revert { output, gas_used } => Err(UnpackError {
            message: format!(
                "This call reverted with output {:#?} and used {} gas.",
                output, gas_used
            ),
            output: Some(output),
        }),
    }
}
