//! Raw SVT-AV1 encoder API. The safe wrapper lives in `dissent-video`.
//!
//! Every item here comes from `Source/API/EbSvtAv1Enc.h` at the commit the submodule pins. The
//! items exist only with the `build` feature.

#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]

#[cfg(feature = "build")]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
