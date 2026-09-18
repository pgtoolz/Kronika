//! Snapshot row collection and cancellation-aware record emission.

use std::collections::{BTreeMap, HashMap, HashSet};

use kronika_reader::{Cell, Dictionary, Row, Segment};
use kronika_registry::ColumnClass;
use serde_json::{Value, json};

use super::{
    CPU_TIME_VIRTUAL_FIELD, CounterReadings, Moments, PROCESS_USER_VIRTUAL_FIELDS, PageContext,
    PageFacts, PageMetadata, PageRankedRow, PageRows, PreparedSnapshot, ProcessUsers, RateContext,
    Readings, RowCoordinate, RowWindow, SNAPSHOT_CHUNK_ROWS, SectionPlans, SnapshotCursor,
    SnapshotViewSpec, StagedRow, cgroup, identity_of, output_rate_fields, projected_rate_columns,
    rate, retained_dictionary, row_timestamp, scheduled_ticks,
};

use crate::projection::{Plan, resolved_dictionary};
use crate::render::{cell, projected_layout, record, shorten};
use crate::{Order, QueryError};

impl PreparedSnapshot {
    pub(super) fn stream_with(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        if cancelled()
            || !emit(record(json!({
                "record": "snapshot",
                "segment": { "id": self.anchor.id().to_string() },
                "at": self.at.to_string(),
            }))?)
        {
            return Ok(());
        }
        if self.first_match_query_id.is_some() {
            return self.emit_first_match(emit, cancelled);
        }
        if self.page_size.is_some() {
            if self.group.is_some() {
                return self.emit_relation_page(emit, cancelled);
            }
            return self.emit_page(emit, cancelled);
        }
        let mut facts = HashMap::new();
        for section in &self.sections {
            if SnapshotViewSpec::for_logical_name(&section.logical_name).is_some()
                || cgroup::legacy(&section.logical_name).is_some()
                || (self.latest && section.plans.iter().all(|plan| plan.timestamp.is_some()))
            {
                if !self.emit_partitioned_section(section, emit, cancelled)? {
                    return Ok(());
                }
            } else {
                for (layout_index, plan) in section.plans.iter().enumerate() {
                    if cancelled() {
                        return Ok(());
                    }
                    if !self.emit_section(
                        section,
                        layout_index,
                        plan,
                        emit,
                        cancelled,
                        &mut facts,
                    )? {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn emit_partitioned_section(
        &self,
        section: &SectionPlans,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(false);
            }
        }
        let contexts = self.page_contexts(section, cancelled)?;
        for context in &contexts {
            if self.row_ordinal.is_some() && context.source.id() != self.anchor.id() {
                continue;
            }
            if cancelled() || !self.emit_context_rows(context, self.row_ordinal, emit, cancelled)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn emit_context_rows(
        &self,
        context: &PageContext<'_>,
        row_ordinal: Option<u64>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let (start_row, row_count) = row_ordinal.map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let source = self.dataset.open(context.source)?;
        let process_users = ProcessUsers::load(&source, context.plan)?;
        let selection_dictionary = context.plan.exact_filter_dictionary(&source)?;
        #[cfg(test)]
        if context.plan.needs_selection_dictionary() {
            CONTEXT_SELECTION_DICTIONARIES.set(CONTEXT_SELECTION_DICTIONARIES.get() + 1);
        }
        let mut chunk = Vec::with_capacity(SNAPSHOT_CHUNK_ROWS);
        let mut failure = None;
        let mut connected = true;
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                if context.window.matches(&row) {
                    chunk.push((ordinal, row));
                }
                if chunk.len() == SNAPSHOT_CHUNK_ROWS {
                    match self.emit_context_chunk(
                        context,
                        &source,
                        &process_users,
                        &selection_dictionary,
                        &mut chunk,
                        emit,
                        cancelled,
                    ) {
                        Ok(still_connected) => connected = still_connected,
                        Err(error) => failure = Some(error),
                    }
                }
                connected && failure.is_none() && !cancelled()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if cancelled() || !connected {
            return Ok(false);
        }
        if chunk.is_empty() {
            return Ok(true);
        }
        self.emit_context_chunk(
            context,
            &source,
            &process_users,
            &selection_dictionary,
            &mut chunk,
            emit,
            cancelled,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "chunk emission carries the exact source, dictionaries, output sink, and cancellation boundary"
    )]
    pub(super) fn emit_context_chunk(
        &self,
        context: &PageContext<'_>,
        source: &Segment,
        process_users: &ProcessUsers,
        selection_dictionary: &Dictionary,
        rows: &mut Vec<(u64, Row)>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        #[cfg(test)]
        CONTEXT_CHUNK_ROWS.set(CONTEXT_CHUNK_ROWS.get().max(rows.len()));
        let mut staged = Vec::new();
        for (ordinal, row) in rows.drain(..) {
            context
                .plan
                .validate_exact_filter_ids(&row, selection_dictionary)?;
            if !context.plan.matches(&row, selection_dictionary) {
                continue;
            }
            let Some(identity) = identity_of(context.plan, &row) else {
                continue;
            };
            staged.push(StagedRow {
                ordinal,
                row,
                identity,
            });
        }
        #[cfg(test)]
        CONTEXT_STAGED_ROWS.set(CONTEXT_STAGED_ROWS.get().saturating_add(staged.len()));
        if staged.is_empty() {
            return Ok(true);
        }
        let dictionary = retained_dictionary(source, &staged)?;
        for staged in staged {
            let before = context.predecessor(&staged.row, &staged.identity);
            let value = Self::row_record(
                context.plan,
                &staged.row,
                before,
                context.elapsed_for(&staged.row),
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: staged.ordinal,
                },
                &dictionary,
                process_users,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn emit_section(
        &self,
        section: &SectionPlans,
        layout_index: usize,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
        facts: &mut HashMap<i64, PageFacts>,
    ) -> Result<bool, QueryError> {
        if !Self::emit_layout(section, plan, emit)? {
            return Ok(false);
        }
        if !plan.applies() {
            return Ok(true);
        }
        let Some(timestamp) = plan.timestamp else {
            return self.emit_untimed(plan, emit, cancelled);
        };
        for context in
            self.timed_contexts(section, layout_index, plan, timestamp, cancelled, facts)?
        {
            if self.row_ordinal.is_some() && context.source.id() != self.anchor.id() {
                continue;
            }
            if cancelled()
                || !self.emit_context_rows(&context, self.row_ordinal, emit, cancelled)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn emit_layout(
        section: &SectionPlans,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
    ) -> Result<bool, QueryError> {
        let fields = plan
            .fields
            .iter()
            .map(|field| {
                (
                    field.name.as_str(),
                    field.column.and_then(|name| plan.contract.column(name)),
                )
            })
            .collect::<Vec<_>>();
        let mut layout = projected_layout(&section.logical_name, plan.contract, &fields);
        if section.logical_name == "os_process"
            && let Some(columns) = layout.get_mut("columns").and_then(Value::as_array_mut)
        {
            for column in columns {
                let Some(name) = column.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if name == CPU_TIME_VIRTUAL_FIELD {
                    *column = json!({
                        "name": name,
                        "type": "i64",
                        "class": "gauge",
                        "unit": "jiffies",
                        "nullable": true,
                        "available": true,
                    });
                } else if PROCESS_USER_VIRTUAL_FIELDS.contains(&name) {
                    *column = json!({
                        "name": name,
                        "type": "dictionary_value",
                        "class": "label",
                        "unit": "none",
                        "nullable": true,
                        "available": true,
                    });
                }
            }
        }
        if cgroup::legacy(&section.logical_name).is_some()
            && let Some(columns) = layout.get_mut("columns").and_then(Value::as_array_mut)
        {
            for column in columns {
                if let Some((ty, unit)) = column
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(cgroup::virtual_type)
                {
                    column["type"] = json!(ty);
                    column["class"] = json!("gauge");
                    column["unit"] = json!(unit);
                    column["nullable"] = json!(true);
                    column["available"] = json!(true);
                }
            }
        }
        Ok(emit(record(json!({
            "record": "layout",
            "layout": layout,
            "rates": output_rate_fields(plan),
        }))?))
    }

    pub(super) fn emit_untimed(
        &self,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let rates = RateContext {
            previous: None,
            elapsed: None,
        };
        let mut rows = Vec::new();
        let source = self.dataset.open(&self.anchor)?;
        source.visit_rows(
            plan.type_id,
            &plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                rows.push((ordinal, row));
                true
            },
        )?;
        if cancelled() {
            return Ok(false);
        }
        self.emit_rows(&source, plan, rows, rates, emit, cancelled)
    }

    pub(super) fn emit_rows(
        &self,
        source: &Segment,
        plan: &Plan,
        rows: Vec<(u64, Row)>,
        rates: RateContext<'_>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let selection_dictionary = plan.selection_dictionary(source, &rows)?;
        let mut staged = Vec::with_capacity(rows.len());
        for (ordinal, row) in rows {
            if !plan.matches(&row, &selection_dictionary) {
                continue;
            }
            let Some(identity) = identity_of(plan, &row) else {
                continue;
            };
            staged.push(StagedRow {
                ordinal,
                row,
                identity,
            });
        }
        self.emit_staged_rows(source, plan, staged, rates, emit, cancelled)
    }

    pub(super) fn emit_staged_rows(
        &self,
        source: &Segment,
        plan: &Plan,
        staged: Vec<StagedRow>,
        rates: RateContext<'_>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let dictionary = retained_dictionary(source, &staged)?;
        let process_users = ProcessUsers::load(source, plan)?;
        for staged in staged {
            let before = rates
                .previous
                .and_then(|previous| previous.get(&staged.identity));
            let value = Self::row_record(
                plan,
                &staged.row,
                before,
                rates.elapsed,
                RowCoordinate {
                    segment_id: source.id(),
                    ordinal: staged.ordinal,
                },
                &dictionary,
                &process_users,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn emit_page(
        &self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(());
            }
        }
        let contexts = self.page_contexts(section, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let process_users = contexts
            .iter()
            .map(|context| {
                let source = self.dataset.open(context.source)?;
                ProcessUsers::load(&source, context.plan)
                    .map(|users| (context.context_index, users))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let anchor = self
            .cursor
            .map(|cursor| self.cursor_anchor(&contexts, &process_users, cursor))
            .transpose()?;
        let page_size = self.page_size.ok_or(QueryError::BadCursor)?;
        let mut page = PageRows::new(page_size.saturating_add(1));
        let mut eligible = 0_u64;
        let mut excluded = 0_u64;
        for context in &contexts {
            self.scan_page(
                context,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                anchor.as_ref(),
                &mut page,
                &mut eligible,
                &mut excluded,
                cancelled,
            )?;
            if cancelled() {
                return Ok(());
            }
        }
        let mut ranked = page.finish();
        let has_more = ranked.len() > page_size;
        let next_cursor = has_more.then(|| {
            let row = &ranked[page_size].staged;
            SnapshotCursor {
                segment_id: self.anchor.id(),
                active_position: self.anchor.active_position().unwrap_or(0),
                context_index: row.context_index,
                ordinal: row.ordinal,
                binding: self.binding,
            }
            .encode()
        });
        ranked.truncate(page_size);
        let returned = ranked.len();
        if !self.emit_page_rows(&contexts, &process_users, ranked, emit, cancelled)? {
            return Ok(());
        }
        Self::emit_page_trailer(
            section,
            &contexts,
            &PageMetadata {
                eligible,
                excluded,
                returned,
                has_more,
                next_cursor,
                page_size,
            },
            self.direction,
            emit,
        )?;
        Ok(())
    }
    pub(super) fn emit_first_match(
        &self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let wanted = self.first_match_query_id.ok_or(QueryError::BadCursor)?;
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(());
            }
        }
        let contexts = self.first_match_contexts(section, cancelled)?;
        let mut returned = 0_usize;
        for context in &contexts {
            let Some((ordinal, row, dictionary)) =
                self.first_match_row(context, wanted, cancelled)?
            else {
                if cancelled() {
                    return Ok(());
                }
                continue;
            };
            let value = Self::row_record(
                context.plan,
                &row,
                None,
                None,
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal,
                },
                &dictionary,
                &ProcessUsers::default(),
                None,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(());
            }
            returned = 1;
            break;
        }
        Self::emit_page_trailer(
            section,
            &contexts,
            &PageMetadata {
                eligible: u64::from(returned != 0),
                excluded: 0,
                returned,
                has_more: false,
                next_cursor: None,
                page_size: 1,
            },
            self.direction,
            emit,
        )
    }

    pub(super) fn first_match_row(
        &self,
        context: &PageContext<'_>,
        wanted: i64,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Option<(u64, Row, Dictionary)>, QueryError> {
        let query_id = context
            .plan
            .contract
            .column("queryid")
            .ok_or_else(|| QueryError::BadFilter("first_match".to_owned()))?
            .name;
        let query = context
            .plan
            .contract
            .column("query")
            .ok_or_else(|| QueryError::BadFilter("first_match".to_owned()))?
            .name;
        let mut projection = vec![query_id, query];
        if let RowWindow::Shared { timestamp, .. } = context.window {
            projection.push(timestamp);
        }
        projection.sort_unstable();
        projection.dedup();

        let source = self.dataset.open(context.source)?;
        let mut offset = 0;
        while offset < context.rows && !cancelled() {
            let mut candidate = None;
            source.visit_rows(
                context.plan.type_id,
                &projection,
                offset,
                usize::MAX,
                |ordinal, row| {
                    #[cfg(test)]
                    FIRST_MATCH_ROWS.set(FIRST_MATCH_ROWS.get() + 1);
                    if context.window.matches(&row)
                        && matches!(row.get(query_id), Some(Cell::I64(stored)) if *stored == wanted)
                    {
                        candidate = Some((ordinal, row));
                        return false;
                    }
                    !cancelled()
                },
            )?;
            let Some((ordinal, row)) = candidate else {
                return Ok(None);
            };
            offset = ordinal.checked_add(1).unwrap_or(context.rows);
            let Some(Cell::StrId(id)) = row.get(query) else {
                continue;
            };
            let dictionary = resolved_dictionary(&source, &HashSet::from([*id]))?;
            let resolved = dictionary.resolve(*id).ok_or_else(|| {
                QueryError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unresolved dictionary id {id}"),
                )))
            })?;
            if resolved.stored_bytes().is_empty() {
                continue;
            }
            return Ok(Some((ordinal, row, dictionary)));
        }
        Ok(None)
    }

    pub(super) fn emit_page_rows(
        &self,
        contexts: &[PageContext<'_>],
        process_users: &HashMap<usize, ProcessUsers>,
        ranked: Vec<PageRankedRow>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let mut ids_by_context: HashMap<usize, HashSet<u64>> = HashMap::new();
        for ranked in &ranked {
            for (_name, value) in ranked.staged.row.iter() {
                if let Cell::StrId(id) = value {
                    ids_by_context
                        .entry(ranked.staged.context_index)
                        .or_default()
                        .insert(*id);
                }
            }
        }
        let dictionaries = contexts
            .iter()
            .map(|context| {
                let ids = ids_by_context
                    .get(&context.context_index)
                    .cloned()
                    .unwrap_or_default();
                let source = self.dataset.open(context.source)?;
                resolved_dictionary(&source, &ids)
                    .map(|dictionary| (context.context_index, dictionary))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        for ranked in ranked {
            let context = contexts
                .iter()
                .find(|context| context.context_index == ranked.staged.context_index)
                .ok_or(QueryError::BadCursor)?;
            let dictionary = dictionaries
                .get(&context.context_index)
                .ok_or(QueryError::BadCursor)?;
            let before = context.predecessor(&ranked.staged.row, &ranked.staged.identity);
            let value = Self::row_record(
                context.plan,
                &ranked.staged.row,
                before,
                context.elapsed_for(&ranked.staged.row),
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: ranked.staged.ordinal,
                },
                dictionary,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn emit_page_trailer(
        section: &SectionPlans,
        contexts: &[PageContext<'_>],
        metadata: &PageMetadata,
        direction: Order,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
    ) -> Result<(), QueryError> {
        let mut order_by = Vec::new();
        for context in contexts {
            if let Some(order) = &context.order
                && !order_by.contains(&order.name)
            {
                order_by.push(order.name);
            }
        }
        let from = contexts
            .iter()
            .filter_map(|context| context.sample_from)
            .min();
        let to = contexts
            .iter()
            .filter_map(|context| context.sample_to)
            .max();
        let _connected = emit(record(json!({
            "record": "snapshot_page",
            "logical_name": section.logical_name,
            "eligible": metadata.eligible.to_string(),
            "excluded": metadata.excluded.to_string(),
            "returned": metadata.returned.to_string(),
            "has_more": metadata.has_more,
            "truncated": metadata.eligible > metadata.returned as u64,
            "next_cursor": metadata.next_cursor,
            "page_size": metadata.page_size,
            "order_by": order_by,
            "order_direction": direction.as_str(),
            "from": from.map(|value| value.to_string()),
            "to": to.map(|value| value.to_string()),
        }))?);
        Ok(())
    }
    pub(super) fn moments(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        at: i64,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Option<Moments>, QueryError> {
        let mut current: Option<i64> = None;
        let mut previous: Option<i64> = None;
        segment.visit_rows(
            plan.type_id,
            &[timestamp],
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                let Some(stored) = row_timestamp(&row, timestamp) else {
                    return true;
                };
                if stored > at {
                    return true;
                }
                match current {
                    Some(chosen) if stored == chosen => {}
                    Some(chosen) if stored > chosen => {
                        previous = Some(chosen);
                        current = Some(stored);
                    }
                    Some(chosen)
                        if previous.is_none_or(|before| stored > before) && stored < chosen =>
                    {
                        previous = Some(stored);
                    }
                    Some(_) => {}
                    None => current = Some(stored),
                }
                true
            },
        )?;
        Ok(current.map(|current| Moments { current, previous }))
    }

    pub(super) fn collect(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        at: i64,
        extra_columns: &[&'static str],
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Readings, QueryError> {
        let mut collected = BTreeMap::new();
        let counters = projected_rate_columns(plan);
        let mut counters = counters;
        if plan.contract.column("starttime").is_some() {
            counters.push("starttime");
        }
        for column in extra_columns {
            if plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
                && !counters.contains(column)
            {
                counters.push(column);
            }
        }
        if counters.is_empty() {
            return Ok(collected);
        }
        let mut projection = counters.clone();
        projection.extend(plan.contract.identity.iter().copied());
        projection.push(timestamp);
        projection.sort_unstable();
        projection.dedup();
        segment.visit_rows(plan.type_id, &projection, 0, usize::MAX, |_ordinal, row| {
            if cancelled() {
                return false;
            }
            if row_timestamp(&row, timestamp) != Some(at) {
                return true;
            }
            let Some(key) = identity_of(plan, &row) else {
                return true;
            };
            let stored = counters
                .iter()
                .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
                .collect();
            collected.insert(key, stored);
            true
        })?;
        if cancelled() {
            collected.clear();
            return Ok(collected);
        }
        Ok(collected)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one row renderer combines physical values, rates, coordinates, and segment-local reference data"
    )]
    pub(super) fn row_record(
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        coordinate: RowCoordinate,
        dictionary: &Dictionary,
        process_users: &ProcessUsers,
        text_limit: Option<u64>,
    ) -> Result<Value, QueryError> {
        let stamped = plan.timestamp.and_then(|column| row_timestamp(row, column));
        let mut values = Vec::with_capacity(plan.fields.len());
        for field in &plan.fields {
            let Some(column) = field.column else {
                if let Some(value) =
                    cgroup::virtual_value(&field.name, row, before.filter(|_| elapsed.is_some()))
                {
                    values.push(value);
                    continue;
                }
                if field.name == CPU_TIME_VIRTUAL_FIELD {
                    values.push(scheduled_ticks(row));
                    continue;
                }
                let uid_column = match field.name.as_str() {
                    "user" => Some("uid"),
                    "effective_user" => Some("euid"),
                    _ => None,
                };
                values.push(
                    uid_column
                        .and_then(|column| process_users.for_row(row, column))
                        .map_or(Value::Null, |name| Value::String(name.to_owned())),
                );
                continue;
            };
            let stored = row.get(column);
            let is_rate = plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
            let exact_plan_calls =
                matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001) && field.name == "calls";
            if is_rate && !exact_plan_calls {
                values.push(rate(stored, before, column, elapsed));
                continue;
            }
            let rendered = match stored {
                Some(stored) => cell(stored, dictionary)?,
                None => Value::Null,
            };
            values.push(match text_limit {
                Some(limit) => shorten(rendered, limit),
                None => rendered,
            });
        }
        Ok(json!({
            "record": "row",
            "type_id": plan.type_id.to_string(),
            "ordinal": coordinate.ordinal.to_string(),
            "segment_id": coordinate.segment_id.to_string(),
            "timestamp": stamped.map(|stored| stored.to_string()),
            "values": values,
        }))
    }
}

#[cfg(test)]
use super::{
    CONTEXT_CHUNK_ROWS, CONTEXT_SELECTION_DICTIONARIES, CONTEXT_STAGED_ROWS, FIRST_MATCH_ROWS,
};
