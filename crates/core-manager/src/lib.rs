/*
 * Copyright 2024 Fluence DAO
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#![feature(slice_take)]

extern crate core;

pub type Map<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub(crate) type MultiMap<K, V> = multimap::MultiMap<K, V, BuildHasherDefault<FxHasher>>;
pub(crate) type BiMap<K, V> =
    bimap::BiHashMap<K, V, BuildHasherDefault<FxHasher>, BuildHasherDefault<FxHasher>>;

pub mod errors;

pub mod types;

mod core_range;

mod dev;

mod dummy;

mod manager;
mod persistence;
mod strict;

pub use ccp_shared::types::CUID;
pub use core_range::CoreRange;
pub use cpu_utils::LogicalCoreId;
pub use cpu_utils::PhysicalCoreId;
pub use dev::DevCoreManager;
pub use dummy::DummyCoreManager;
use fxhash::FxHasher;
pub use manager::CoreManager;
pub use manager::CoreManagerFunctions;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
pub use strict::StrictCoreManager;
