// Copyright (C) 2019-2021 Aleo Systems Inc.
// This file is part of the Aleo library.

// The Aleo library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The Aleo library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the Aleo library. If not, see <https://www.gnu.org/licenses/>.

#![forbid(unsafe_code)]

mod execute;
mod rest;
mod scan;

use anyhow::{anyhow, bail, Result};
use core::{convert::TryInto, ops::Range};
use reqwest::Url;

use snarkvm::prelude::{
    Block,
    BlockMemory,
    Ciphertext,
    ConsensusMemory,
    ConsensusStore,
    Field,
    Identifier,
    Network,
    PrivateKey,
    Program,
    ProgramID,
    Query,
    Record,
    Response,
    Testnet3,
    Transaction,
    Value,
    ViewKey,
    VM,
};

#[derive(Clone)]
pub struct Client<N: Network> {
    client: reqwest::Client,
    base_url: Url,
    vm: VM<N, ConsensusMemory<N>>,
}

impl<N: Network> Client<N> {
    /// Initializes a new client with the given node URL.
    pub fn new(node_url: &str) -> Result<Self> {
        // Initialize a new in-memory store.
        let store = ConsensusStore::open(None)?;
        // Initialize a new client.
        Ok(Self { client: reqwest::Client::new(), base_url: Url::parse(node_url)?, vm: VM::from(store)? })
    }

    /// Returns the node URL.
    pub fn node_url(&self) -> &str {
        self.base_url.as_str()
    }

    /// Returns the underlying REST client.
    pub fn inner(&self) -> &reqwest::Client {
        &self.client
    }
}
