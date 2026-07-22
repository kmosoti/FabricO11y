//! PyO3 exposes only coarse JSON conversion around `fabric_sdk::Engine`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use fabric_sdk::{Engine, SdkError, SdkErrorCategory};
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use serde::Serialize;

create_exception!(_fabrico11y, FabricError, PyException, "FabricO11y error.");
create_exception!(
    _fabrico11y,
    AdmissionError,
    FabricError,
    "Admission or contract conversion failed."
);
create_exception!(
    _fabrico11y,
    SemanticError,
    FabricError,
    "Deterministic semantic validation failed."
);
create_exception!(
    _fabrico11y,
    StorageError,
    FabricError,
    "Storage, catalog, or recovery operation failed."
);
create_exception!(
    _fabrico11y,
    IntegrityError,
    StorageError,
    "Persisted evidence failed an integrity invariant."
);

#[pyclass(name = "Engine", unsendable)]
struct PyEngine {
    inner: Engine,
}

#[pymethods]
impl PyEngine {
    #[new]
    fn new(root: &str) -> PyResult<Self> {
        boundary(|| Engine::open(root).map(|inner| Self { inner }))
    }

    fn admit_json(&mut self, record_json: &str) -> PyResult<String> {
        boundary(|| {
            let receipt = self.inner.admit_json(record_json)?;
            to_json(&receipt)
        })
    }

    fn seal_json(&mut self) -> PyResult<String> {
        boundary(|| {
            let receipt = self.inner.seal()?;
            to_json(&receipt)
        })
    }

    fn replay_json(&self) -> PyResult<String> {
        boundary(|| self.inner.replay_json())
    }

    fn validate_json(&self) -> PyResult<String> {
        boundary(|| {
            let report = self.inner.validate()?;
            to_json(&report)
        })
    }

    fn locate_json(&self, record_id: &str) -> PyResult<String> {
        boundary(|| {
            let location = self.inner.locate(record_id)?;
            to_json(&location)
        })
    }

    fn recovery_json(&self) -> PyResult<String> {
        boundary(|| to_json(self.inner.recovery_report()))
    }

    fn pending_count(&self) -> usize {
        self.inner.pending_count()
    }
}

fn to_json(value: &impl Serialize) -> Result<String, SdkError> {
    let value =
        serde_json::to_value(value).map_err(|error| SdkError::JsonInvalid(error.to_string()))?;
    serde_json::to_string(&value).map_err(|error| SdkError::JsonInvalid(error.to_string()))
}

fn boundary<T>(operation: impl FnOnce() -> Result<T, SdkError>) -> PyResult<T> {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result.map_err(sdk_error_to_python),
        Err(_) => Err(panic_error()),
    }
}

fn sdk_error_to_python(error: SdkError) -> PyErr {
    let category = category_name(error.category());
    let message = error.to_string();
    let python_error = match error.category() {
        SdkErrorCategory::JsonInvalid
        | SdkErrorCategory::SchemaInvalid
        | SdkErrorCategory::AdmissionRejected
        | SdkErrorCategory::AdmissionInvalid => AdmissionError::new_err(message),
        SdkErrorCategory::SemanticInvalid => SemanticError::new_err(message),
        SdkErrorCategory::CatalogDisagreement | SdkErrorCategory::StagingCorrupt => {
            IntegrityError::new_err(message)
        }
        SdkErrorCategory::Io
        | SdkErrorCategory::ClockFailed
        | SdkErrorCategory::SegmentInvalid
        | SdkErrorCategory::MissingDictionary
        | SdkErrorCategory::CatalogInvalid
        | SdkErrorCategory::NoPendingRecords
        | SdkErrorCategory::UnexpectedStorageEntry
        | SdkErrorCategory::RecoveryRequired => StorageError::new_err(message),
    };
    attach_category(python_error, category)
}

fn panic_error() -> PyErr {
    attach_category(
        FabricError::new_err("Rust panic contained at the Python boundary"),
        "panic_contained",
    )
}

fn attach_category(error: PyErr, category: &str) -> PyErr {
    match Python::attach(|py| error.value(py).setattr("category", category)) {
        Ok(()) => error,
        Err(attribute_error) => attribute_error,
    }
}

fn category_name(category: SdkErrorCategory) -> &'static str {
    match category {
        SdkErrorCategory::Io => "io",
        SdkErrorCategory::JsonInvalid => "json_invalid",
        SdkErrorCategory::SchemaInvalid => "schema_invalid",
        SdkErrorCategory::SemanticInvalid => "semantic_invalid",
        SdkErrorCategory::AdmissionRejected => "admission_rejected",
        SdkErrorCategory::AdmissionInvalid => "admission_invalid",
        SdkErrorCategory::ClockFailed => "clock_failed",
        SdkErrorCategory::SegmentInvalid => "segment_invalid",
        SdkErrorCategory::MissingDictionary => "missing_dictionary",
        SdkErrorCategory::CatalogInvalid => "catalog_invalid",
        SdkErrorCategory::CatalogDisagreement => "catalog_disagreement",
        SdkErrorCategory::StagingCorrupt => "staging_corrupt",
        SdkErrorCategory::NoPendingRecords => "no_pending_records",
        SdkErrorCategory::UnexpectedStorageEntry => "unexpected_storage_entry",
        SdkErrorCategory::RecoveryRequired => "recovery_required",
    }
}

#[pymodule]
fn _fabrico11y(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyEngine>()?;
    module.add("FabricError", module.py().get_type::<FabricError>())?;
    module.add("AdmissionError", module.py().get_type::<AdmissionError>())?;
    module.add("SemanticError", module.py().get_type::<SemanticError>())?;
    module.add("StorageError", module.py().get_type::<StorageError>())?;
    module.add("IntegrityError", module.py().get_type::<IntegrityError>())?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
