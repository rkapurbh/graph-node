extern crate ethereum_types;
extern crate futures;
extern crate serde_json;
#[macro_use]
extern crate slog;
extern crate ethabi;
extern crate graph;
extern crate hex;
extern crate nan_preserving_float;
extern crate num_bigint;
extern crate tokio_core;
extern crate uuid;
extern crate wasmi;
extern crate web3;

mod asc_abi;
mod host;
mod module;
mod to_from;

pub use self::host::{RuntimeHost, RuntimeHostBuilder, RuntimeHostConfig};

#[derive(Clone, Debug)]
pub(crate) struct UnresolvedContractCall {
    pub block_hash: ethereum_types::H256,
    pub contract_name: String,
    pub contract_address: ethereum_types::Address,
    pub function_name: String,
    pub function_args: Vec<ethabi::Token>,
}
