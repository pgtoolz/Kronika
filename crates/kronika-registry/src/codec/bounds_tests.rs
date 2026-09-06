use super::bounds::{
    FinalPlainColumnSize, final_data_body_bound, final_single_batch_plain_body_bound,
    zstd_compress_bound,
};
use super::pg_stat_statements::PgStatStatementsV6;
use super::{
    CodecError, FINAL_FILE_FRAMING_BOUND, FINAL_PAGE_FRAMING_BOUND, MAX_DECODED_SECTION_BYTES,
    MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS, SECTION_WRITE_BATCH_ROWS, check_row_cap,
};
use crate::Section;

#[test]
fn version_one_section_limits_follow_wire_and_container_fields() {
    assert_eq!(
        MAX_SECTION_BYTES,
        usize::try_from(kronika_format::MAX_PHYSICAL_SECTION_BYTES)
            .expect("the one GiB format envelope fits usize")
    );
    assert_eq!(MAX_DECODED_SECTION_BYTES, kronika_format::MAX_JOURNAL_LEN);
    assert_eq!(MAX_SECTION_ROWS, u32::MAX as usize);
    assert_eq!(SECTION_WRITE_BATCH_ROWS, 65_536);
    assert_eq!(MAX_ROW_GROUPS, 65_536);

    assert!(matches!(
        check_row_cap(MAX_SECTION_ROWS + 1),
        Err(CodecError::TooManyRows { rows, max })
            if rows == MAX_SECTION_ROWS + 1 && max == MAX_SECTION_ROWS
    ));
    assert!(matches!(
        final_single_batch_plain_body_bound(
            [FinalPlainColumnSize::new(MAX_SECTION_BYTES, 0)],
            1,
        ),
        Err(CodecError::SectionTooLarge { max, .. }) if max == MAX_SECTION_BYTES
    ));
}

#[test]
fn fifteen_minute_statement_section_is_not_rejected_at_the_old_eight_mib_boundary() {
    const LIVE_SCALE_ROWS: usize = 146_534;
    const OLD_SECTION_BYTES: usize = 8 * 1024 * 1024;
    const { assert!(LIVE_SCALE_ROWS > SECTION_WRITE_BATCH_ROWS) };

    let bound = final_data_body_bound(
        PgStatStatementsV6::CONTRACT.type_id.get(),
        LIVE_SCALE_ROWS,
        0,
    )
    .expect("the fifteen-minute aggregate fits the version-one container envelope");

    assert!(bound > OLD_SECTION_BYTES);
    assert!(bound < MAX_SECTION_BYTES);
}

#[test]
fn single_batch_bound_covers_partitioned_zstd_frames() {
    let partitions = [0, 1, 255, 256, 131_071, 131_072];
    let page_input = partitions.iter().sum::<usize>();
    let pages = partitions.len();
    let bound =
        final_single_batch_plain_body_bound([FinalPlainColumnSize::new(page_input, 0)], pages)
            .expect("partitioned column fits");
    let exact_compression_bounds = partitions
        .into_iter()
        .map(|bytes| zstd_compress_bound(bytes).expect("small frame bound fits"))
        .sum::<usize>();
    let partitioned_bound =
        FINAL_FILE_FRAMING_BOUND + exact_compression_bounds + pages * FINAL_PAGE_FRAMING_BOUND;

    assert!(bound >= partitioned_bound);
}

#[test]
fn single_batch_bound_rejects_zero_pages_and_overflow() {
    assert!(matches!(
        final_single_batch_plain_body_bound([FinalPlainColumnSize::new(1, 0)], 0,),
        Err(CodecError::InvalidPageLayout)
    ));
    assert!(matches!(
        final_single_batch_plain_body_bound([FinalPlainColumnSize::new(usize::MAX, 1)], 1,),
        Err(CodecError::SectionTooLarge { .. })
    ));
}
