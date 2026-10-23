//! The macro that runs every scenario against a store.

/// Generate one `#[test]` per scenario against a store factory, a [`Factory`](super::Factory).
///
/// ```ignore
/// jewelcase_scanner::store_suite!(make);
/// ```
#[macro_export]
macro_rules! store_suite {
    ($make:expr) => {
        $crate::store_suite!(@tests $make;
            cold_scan_then_incremental_is_all_unchanged,
            retag_updates_in_place_and_keeps_identity,
            lyrics_file_beside_an_unchanged_track,
            an_empty_lyrics_file_writes_nothing,
            sidecars_beside_unchanged_tracks_come_and_go,
            a_folder_scan_under_sidecars_finds_its_tracks_unchanged,
            a_changed_file_is_analyzed_again,
            a_lyrics_file_alone_keeps_the_analysis,
            a_result_for_replaced_audio_is_not_stored,
            a_known_file_that_cannot_be_read_keeps_its_track,
            an_unreadable_file_is_read_again_until_it_reads,
            a_broken_file_moved_away_and_back_keeps_its_problem,
            a_file_restored_with_its_old_time_is_read_again,
            a_result_for_a_file_broken_during_its_decode_is_not_stored,
            an_unreadable_replacement_loses_its_analysis,
            a_missing_file_back_unreadable_is_back,
            missing_then_returned,
            unavailable_root_suspends_and_touches_nothing,
            bad_file_is_a_problem_that_clears,
            a_symlink_is_a_problem,
            a_scope_through_a_folder_link_walks_nothing,
            a_scope_naming_a_deleted_folder_reconciles,
            a_deleted_file_takes_its_problem,
            a_scope_at_a_file_reconciles_nothing,
            indexes_under_sees_live_tracks_and_problems,
            a_dangling_link_keeps_its_problem,
            a_problem_on_a_present_file_stays,
            a_folder_problem_clears_once_it_lists,
            an_unreadable_folder_keeps_its_tracks,
            untagged_file_is_named_after_itself,
            an_absent_album_artist_is_the_primary_artist,
            excludes_are_never_indexed,
            folder_scope_reconciles_only_inside_itself,
            non_ascii_subtree_reconciles_only_inside_itself,
            non_ascii_directory_reconciles_only_itself,
            root_directory_reconciles_only_its_own_files,
            scan_times_read_back,
            queue_runs_resumes_and_coalesces,
            a_resumed_scan_takes_no_new_work,
            a_shutdown_leaves_its_scans_to_resume,
            placeholders_fill_once_per_image,
            a_missing_file_waits_until_it_returns,
            a_file_not_permitted_waits_until_it_is,
            an_older_analyzers_results_are_redone,
            an_unopenable_file_does_not_starve_the_next,
            opening_a_file_clears_only_unopened_problems,
            a_cover_moved_away_waits_until_it_returns,
            a_pipe_is_judged_without_blocking,
            a_decoder_that_cannot_start_backs_off,
            a_root_going_away_pauses_without_writing,
            a_file_that_always_fails_does_not_block_the_next,
            a_link_is_judged_once,
            copy_is_a_second_track,
            replaced_is_not_a_move,
            a_stop_flushes_the_batch_in_hand,
            a_root_gone_before_reconcile_suspends_and_marks_nothing,
            an_unchanged_file_reads_no_sidecars,
            sidecars_are_read_once_per_directory,
            a_folder_larger_than_a_batch_flushes_partway,
            a_scans_first_batch_holds_min_batch,
            a_folder_flushes_partway_once_the_interval_passes,
            a_slow_store_is_given_batches_it_applies_in_time,
            a_scope_inside_an_excluded_folder_indexes_nothing,
            a_scope_inside_a_newly_excluded_folder_marks_its_tracks_missing,
            a_linked_scope_inside_an_excluded_folder_reports_nothing,
            an_invalid_pattern_suspends_and_leaves_the_store_untouched,
            a_folder_read_the_store_fails_suspends_the_scan_before_another_write,
            a_problem_list_the_store_fails_suspends_the_scan_counting_what_it_marked_missing,
            a_folder_the_store_cannot_check_counts_as_indexed,
            a_parser_panic_is_a_problem_and_the_scan_completes,
            a_panic_suspends_the_scan_and_the_worker_runs_the_next,
            a_failed_batch_suspends_at_the_last_persisted_cursor,
            a_resumed_scan_that_panics_keeps_its_starting_cursor,
            a_scan_shut_down_partway_through_a_folder_resumes_counting_each_file_once,
            a_scan_suspended_partway_through_a_folder_resumes_counting_each_file_once,
            a_known_file_that_cannot_be_read_is_counted_once_across_a_resume,
            a_failed_batch_leaves_the_scopes_from_its_cursor_undone,
            a_stopped_scan_leaves_nothing_undone,
            an_unlistable_folder_is_counted_once_across_resumes,
            unlistable_folders_count_once_the_cursor_passes_their_scope,
            a_root_gone_after_its_walk_counts_its_last_unlistable_folder_as_the_scan_ends,
            a_renamed_folder_moves_its_tracks,
            a_renamed_file_dirties_only_its_folder,
            deleted_files_and_folders_go_missing,
            an_unchanged_rescan_writes_nothing,
            a_folder_gone_from_its_parents_listing_takes_its_tracks,
            an_unlistable_folder_keeps_its_tracks_across_a_resume,
            a_scan_stopped_partway_through_a_folder_marks_nothing_there,
            a_scan_stopped_partway_through_marking_marks_the_rest_on_resume,
            queue_drains_and_broken_files_do_not_repeat,
        );
        #[test]
        #[ignore = "track identity across moves is open decision 3 (design/general.md §11)"]
        fn move_keeps_identity() {
            $crate::testing::move_keeps_identity($make);
        }
        #[test]
        #[ignore = "track identity across moves is open decision 3 (design/general.md §11)"]
        fn move_and_retag_keeps_identity() {
            $crate::testing::move_and_retag_keeps_identity($make);
        }
    };
    (@tests $make:expr; $($scenario:ident,)*) => {
        $(
            #[test]
            fn $scenario() {
                $crate::testing::$scenario($make);
            }
        )*
    };
}
