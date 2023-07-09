//! Groups together long-running async tasks for best discoverability
//!
//! Examples of such tasks are creating a new wallet; which requires the
//! node to prove `VALID NEW WALLET`, submit the wallet on-chain, wait for
//! transaction success, and then prove `VALID COMMITMENTS`

#![allow(incomplete_features)]
#![deny(unsafe_code)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(missing_docs)]
#![feature(let_chains)]
#![feature(generic_const_exprs)]
#![feature(iter_advance_by)]

pub mod create_new_wallet;
pub mod driver;
mod helpers;
pub mod initialize_state;
pub mod lookup_wallet;
pub mod settle_match;
pub mod settle_match_internal;
pub mod update_wallet;