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

#![feature(try_blocks)]
#![feature(assert_matches)]
#![warn(rust_2018_idioms)]
#![deny(
    dead_code,
    nonstandard_style,
    unused_imports,
    unused_mut,
    unused_variables,
    unused_unsafe,
    unreachable_patterns
)]

#[macro_use]
extern crate fstrings;

mod error;
mod files;
mod modules;

pub use error::ModuleError;
pub use files::{load_blueprint, load_module_by_path, load_module_descriptor};
pub use modules::EffectorsMode;
pub use modules::ModuleRepository;

// reexport
pub use fluence_app_service::{
    TomlMarineModuleConfig as ModuleConfig, TomlMarineNamedModuleConfig as NamedModuleConfig,
    TomlWASIConfig as WASIConfig,
};
pub use fs_utils::list_files;
pub use service_modules::AddBlueprint;
