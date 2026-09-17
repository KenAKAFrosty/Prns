use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use object::{Object, ObjectSection, ObjectSymbol, SectionKind};
use thiserror::Error;

use super::executable::display_symbol;

#[derive(Debug)]
pub(crate) struct AsyncMemoryAnalysis {
    pub(crate) task_pool_bytes: u64,
    pub(crate) task_pools: Vec<TaskPoolAllocation>,
}

#[derive(Debug)]
pub(crate) struct TaskPoolAllocation {
    pub(crate) task: String,
    pub(crate) address: u64,
    pub(crate) bytes: u64,
    pub(crate) section: String,
}

pub(crate) fn analyze(path: &Path) -> Result<AsyncMemoryAnalysis, AsyncMemoryError> {
    let bytes = fs::read(path).map_err(|source| AsyncMemoryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let object =
        object::File::parse(bytes.as_slice()).map_err(|source| AsyncMemoryError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let task_poll_markers = object
        .symbols()
        .filter(|symbol| {
            symbol.is_definition()
                && symbol.kind() == object::SymbolKind::Text
                && symbol.name().ok().map(display_symbol).is_some_and(|name| {
                    name.starts_with("<embassy_executor::raw::TaskStorage<")
                        && name.ends_with(">::poll")
                })
        })
        .filter_map(|symbol| symbol.name().ok().map(display_symbol))
        .collect::<Vec<_>>();
    let mut task_pools = Vec::new();
    for symbol in object.symbols().filter(|symbol| {
        symbol.is_definition() && symbol.kind() == object::SymbolKind::Data && symbol.size() != 0
    }) {
        let Some(raw_name) = symbol.name().ok() else {
            continue;
        };
        let name = display_symbol(raw_name);
        let Some(task) = name.strip_suffix("::POOL") else {
            continue;
        };
        if !has_task_poll(task, &task_poll_markers) {
            continue;
        }
        let index = symbol
            .section_index()
            .ok_or_else(|| AsyncMemoryError::MissingSection {
                symbol: name.clone(),
            })?;
        let section =
            object
                .section_by_index(index)
                .map_err(|source| AsyncMemoryError::Section {
                    symbol: name.clone(),
                    source,
                })?;
        if section.kind() != SectionKind::UninitializedData {
            return Err(AsyncMemoryError::InitializedTaskPool {
                symbol: name,
                section: section.name().unwrap_or("<invalid>").to_string(),
            });
        }
        let section_name =
            section
                .name()
                .map(str::to_string)
                .map_err(|source| AsyncMemoryError::SectionName {
                    symbol: name.clone(),
                    source,
                })?;
        task_pools.push(TaskPoolAllocation {
            task: task.to_string(),
            address: symbol.address(),
            bytes: symbol.size(),
            section: section_name,
        });
    }
    task_pools.sort_by(|left, right| {
        left.address
            .cmp(&right.address)
            .then_with(|| left.task.cmp(&right.task))
    });
    let task_pool_bytes = task_pools
        .iter()
        .try_fold(0_u64, |total, pool| total.checked_add(pool.bytes));
    let task_pool_bytes = task_pool_bytes.ok_or(AsyncMemoryError::TotalOverflow)?;
    if task_pools.is_empty() {
        return Err(AsyncMemoryError::MissingTaskPools {
            path: path.to_path_buf(),
        });
    }
    Ok(AsyncMemoryAnalysis {
        task_pool_bytes,
        task_pools,
    })
}

fn has_task_poll(task: &str, symbols: &[String]) -> bool {
    let Some((module, task_name)) = task.rsplit_once("::") else {
        return false;
    };
    let marker = if task_name == "__embassy_main" {
        format!("TaskStorage<{module}::____embassy_main_task::")
    } else {
        format!("TaskStorage<{module}::__{task_name}_task::")
    };
    symbols.iter().any(|symbol| symbol.contains(&marker))
}

#[derive(Debug, Error)]
pub(crate) enum AsyncMemoryError {
    #[error("could not read firmware ELF {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse firmware ELF {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: object::Error,
    },
    #[error("Embassy task-pool symbol {symbol:?} has no section")]
    MissingSection { symbol: String },
    #[error("could not read section for Embassy task-pool symbol {symbol:?}: {source}")]
    Section {
        symbol: String,
        #[source]
        source: object::Error,
    },
    #[error("could not read section name for Embassy task-pool symbol {symbol:?}: {source}")]
    SectionName {
        symbol: String,
        #[source]
        source: object::Error,
    },
    #[error("Embassy task-pool symbol {symbol:?} is initialized in {section:?}")]
    InitializedTaskPool { symbol: String, section: String },
    #[error("firmware ELF {path} has no production Embassy task-pool allocations")]
    MissingTaskPools { path: PathBuf },
    #[error("production Embassy task-pool byte total overflowed")]
    TotalOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_pools_require_their_exact_generated_task_storage() {
        let symbols = vec![
            "<embassy_executor::raw::TaskStorage<firmware::radio::__run_task::inner>>::poll"
                .to_string(),
            "<embassy_executor::raw::TaskStorage<board::____embassy_main_task::inner>>::poll"
                .to_string(),
            "<embassy_executor::raw::TaskStorage<board::__main::____embassy_main_task::inner>>::poll"
                .to_string(),
        ];

        assert!(has_task_poll("firmware::radio::run", &symbols));
        assert!(has_task_poll("board::__embassy_main", &symbols));
        assert!(has_task_poll("board::__main::__embassy_main", &symbols));
        assert!(!has_task_poll("firmware::display::run", &symbols));
        assert!(!has_task_poll("POOL", &symbols));
    }
}
