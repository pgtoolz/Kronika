//! Heatmap batch errors and common query refusals.

use super::{HeatmapError, HeatmapQueryErrorKind};

use crate::QueryError;

impl HeatmapError {
    pub(super) fn invalid(ranking_index: usize, message: impl Into<String>) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::BadFilter("heatmap".to_owned()),
            Vec::new(),
        )
    }

    pub(super) fn invalid_parameter(
        ranking_index: usize,
        message: impl Into<String>,
        parameter: &str,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::BadFilter(parameter.to_owned()),
            Vec::new(),
        )
    }

    pub(super) fn bad_locator(ranking_index: usize, message: impl Into<String>) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::BadLocator,
            Vec::new(),
        )
    }

    pub(super) fn no_such_section(
        ranking_index: usize,
        message: impl Into<String>,
        valid_options: Vec<String>,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::NoSuchSection,
            valid_options,
        )
    }

    pub(super) fn no_such_column(
        ranking_index: usize,
        message: impl Into<String>,
        column: String,
        valid_options: Vec<String>,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::NoSuchColumn(column),
            valid_options,
        )
    }

    pub(super) fn mixed_units(
        ranking_index: usize,
        message: impl Into<String>,
        fields: String,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::MixedUnits(fields),
            Vec::new(),
        )
    }

    pub(super) fn invalid_as(
        ranking_index: usize,
        message: impl Into<String>,
        query_error: HeatmapQueryErrorKind,
        valid_options: Vec<String>,
    ) -> Self {
        Self {
            ranking_index,
            message: message.into(),
            query_error: Some(query_error),
            valid_options,
            source: None,
        }
    }

    pub(super) fn storage<E>(ranking_index: usize, error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            ranking_index,
            message: error.to_string(),
            query_error: None,
            valid_options: Vec::new(),
            source: Some(Box::new(error)),
        }
    }

    pub(super) fn failure(ranking_index: usize, message: impl Into<String>) -> Self {
        Self {
            ranking_index,
            message: message.into(),
            query_error: None,
            valid_options: Vec::new(),
            source: None,
        }
    }

    /// Zero-based expanded-item index that refused the batch.
    #[must_use]
    pub const fn ranking_index(&self) -> usize {
        self.ranking_index
    }

    /// Registry names valid in place of an unknown section or field.
    #[must_use]
    pub fn valid_options(&self) -> &[String] {
        &self.valid_options
    }

    /// Reduce this indexed batch error to the common query error family.
    #[must_use]
    pub fn into_query(mut self) -> QueryError {
        match self.query_error.take() {
            Some(HeatmapQueryErrorKind::BadFilter(parameter)) => QueryError::BadFilter(parameter),
            Some(HeatmapQueryErrorKind::BadLocator) => QueryError::BadLocator(self.message),
            Some(HeatmapQueryErrorKind::NoSuchSection) => QueryError::NoSuchSection,
            Some(HeatmapQueryErrorKind::NoSuchColumn(column)) => QueryError::NoSuchColumn(column),
            Some(HeatmapQueryErrorKind::MixedUnits(fields)) => QueryError::MixedUnits(fields),
            None => QueryError::Unreadable(Box::new(self)),
        }
    }
}

impl std::fmt::Display for HeatmapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rankings[{}]: {}", self.ranking_index, self.message)
    }
}

impl std::error::Error for HeatmapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let source: &(dyn std::error::Error + 'static) = self.source.as_deref()?;
        Some(source)
    }
}
