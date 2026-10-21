//! The macro that runs every scenario against a store.

/// Generate one `#[test]` per scenario against a store factory.
///
/// ```ignore
/// jewelcase_scanner::store_suite!(|_root| MemoryStore::new());
/// ```
#[macro_export]
macro_rules! store_suite {
    ($make:expr) => {
        #[test]
        fn cold_scan_then_incremental_is_all_unchanged() {
            $crate::testing::cold_scan_then_incremental_is_all_unchanged($make);
        }
        #[test]
        fn retag_updates_in_place_and_keeps_identity() {
            $crate::testing::retag_updates_in_place_and_keeps_identity($make);
        }
        #[test]
        fn lyrics_file_beside_an_unchanged_track() {
            $crate::testing::lyrics_file_beside_an_unchanged_track($make);
        }
        #[test]
        fn a_changed_file_is_analyzed_again() {
            $crate::testing::a_changed_file_is_analyzed_again($make);
        }
        #[test]
        fn a_lyrics_file_alone_keeps_the_analysis() {
            $crate::testing::a_lyrics_file_alone_keeps_the_analysis($make);
        }
        #[test]
        fn a_result_for_replaced_audio_is_not_stored() {
            $crate::testing::a_result_for_replaced_audio_is_not_stored($make);
        }
        #[test]
        fn a_known_file_that_cannot_be_read_keeps_its_track() {
            $crate::testing::a_known_file_that_cannot_be_read_keeps_its_track($make);
        }
        #[test]
        fn an_unreadable_file_is_read_again_until_it_reads() {
            $crate::testing::an_unreadable_file_is_read_again_until_it_reads($make);
        }
        #[test]
        fn a_broken_file_moved_away_and_back_keeps_its_problem() {
            $crate::testing::a_broken_file_moved_away_and_back_keeps_its_problem($make);
        }
        #[test]
        fn a_file_restored_with_its_old_time_is_read_again() {
            $crate::testing::a_file_restored_with_its_old_time_is_read_again($make);
        }
        #[test]
        fn a_result_for_a_file_broken_during_its_decode_is_not_stored() {
            $crate::testing::a_result_for_a_file_broken_during_its_decode_is_not_stored($make);
        }
        #[test]
        fn an_unreadable_replacement_loses_its_analysis() {
            $crate::testing::an_unreadable_replacement_loses_its_analysis($make);
        }
        #[test]
        fn a_missing_file_back_unreadable_is_back() {
            $crate::testing::a_missing_file_back_unreadable_is_back($make);
        }
        #[test]
        fn missing_then_returned() {
            $crate::testing::missing_then_returned($make);
        }
        #[test]
        fn unavailable_root_suspends_and_touches_nothing() {
            $crate::testing::unavailable_root_suspends_and_touches_nothing($make);
        }
        #[test]
        fn bad_file_is_a_problem_that_clears() {
            $crate::testing::bad_file_is_a_problem_that_clears($make);
        }
        #[test]
        fn a_symlink_is_a_problem() {
            $crate::testing::a_symlink_is_a_problem($make);
        }
        #[test]
        fn a_scope_through_a_folder_link_walks_nothing() {
            $crate::testing::a_scope_through_a_folder_link_walks_nothing($make);
        }
        #[test]
        fn a_scope_naming_a_deleted_folder_reconciles() {
            $crate::testing::a_scope_naming_a_deleted_folder_reconciles($make);
        }
        #[test]
        fn a_deleted_file_takes_its_problem() {
            $crate::testing::a_deleted_file_takes_its_problem($make);
        }
        #[test]
        fn a_scope_at_a_file_reconciles_nothing() {
            $crate::testing::a_scope_at_a_file_reconciles_nothing($make);
        }
        #[test]
        fn indexes_under_sees_live_tracks_and_problems() {
            $crate::testing::indexes_under_sees_live_tracks_and_problems($make);
        }
        #[test]
        fn a_dangling_link_keeps_its_problem() {
            $crate::testing::a_dangling_link_keeps_its_problem($make);
        }
        #[test]
        fn a_problem_on_a_present_file_stays() {
            $crate::testing::a_problem_on_a_present_file_stays($make);
        }
        #[test]
        fn a_folder_problem_clears_once_it_lists() {
            $crate::testing::a_folder_problem_clears_once_it_lists($make);
        }
        #[test]
        fn an_unreadable_folder_keeps_its_tracks() {
            $crate::testing::an_unreadable_folder_keeps_its_tracks($make);
        }
        #[test]
        fn untagged_file_is_named_after_itself() {
            $crate::testing::untagged_file_is_named_after_itself($make);
        }
        #[test]
        fn excludes_are_never_indexed() {
            $crate::testing::excludes_are_never_indexed($make);
        }
        #[test]
        fn folder_scope_reconciles_only_inside_itself() {
            $crate::testing::folder_scope_reconciles_only_inside_itself($make);
        }
        #[test]
        fn non_ascii_subtree_reconciles_only_inside_itself() {
            $crate::testing::non_ascii_subtree_reconciles_only_inside_itself($make);
        }
        #[test]
        fn non_ascii_directory_reconciles_only_itself() {
            $crate::testing::non_ascii_directory_reconciles_only_itself($make);
        }
        #[test]
        fn root_directory_reconciles_only_its_own_files() {
            $crate::testing::root_directory_reconciles_only_its_own_files($make);
        }
        #[test]
        fn scan_times_read_back() {
            $crate::testing::scan_times_read_back($make);
        }
        #[test]
        fn queue_runs_resumes_and_coalesces() {
            $crate::testing::queue_runs_resumes_and_coalesces($make);
        }
        #[test]
        fn a_resumed_scan_takes_no_new_work() {
            $crate::testing::a_resumed_scan_takes_no_new_work($make);
        }
        #[test]
        fn a_reconfigure_during_a_scan_applies_its_excludes() {
            $crate::testing::a_reconfigure_during_a_scan_applies_its_excludes($make);
        }
        #[test]
        fn a_shutdown_leaves_its_scans_to_resume() {
            $crate::testing::a_shutdown_leaves_its_scans_to_resume($make);
        }
        #[test]
        fn a_cancelled_scan_stays_cancelled() {
            $crate::testing::a_cancelled_scan_stays_cancelled($make);
        }
        #[test]
        fn placeholders_fill_once_per_image() {
            $crate::testing::placeholders_fill_once_per_image($make);
        }
        #[test]
        fn a_missing_file_waits_until_it_returns() {
            $crate::testing::a_missing_file_waits_until_it_returns($make);
        }
        #[test]
        fn a_file_not_permitted_waits_until_it_is() {
            $crate::testing::a_file_not_permitted_waits_until_it_is($make);
        }
        #[test]
        fn an_older_analyzers_results_are_redone() {
            $crate::testing::an_older_analyzers_results_are_redone($make);
        }
        #[test]
        fn an_unopenable_file_does_not_starve_the_next() {
            $crate::testing::an_unopenable_file_does_not_starve_the_next($make);
        }
        #[test]
        fn opening_a_file_clears_only_unopened_problems() {
            $crate::testing::opening_a_file_clears_only_unopened_problems($make);
        }
        #[test]
        fn a_cover_moved_away_waits_until_it_returns() {
            $crate::testing::a_cover_moved_away_waits_until_it_returns($make);
        }
        #[test]
        fn a_pipe_is_judged_without_blocking() {
            $crate::testing::a_pipe_is_judged_without_blocking($make);
        }
        #[test]
        fn a_decoder_that_cannot_start_backs_off() {
            $crate::testing::a_decoder_that_cannot_start_backs_off($make);
        }
        #[test]
        fn a_root_going_away_pauses_without_writing() {
            $crate::testing::a_root_going_away_pauses_without_writing($make);
        }
        #[test]
        fn a_file_that_always_fails_does_not_block_the_next() {
            $crate::testing::a_file_that_always_fails_does_not_block_the_next($make);
        }
        #[test]
        fn a_link_is_judged_once() {
            $crate::testing::a_link_is_judged_once($make);
        }
        #[test]
        fn copy_is_a_second_track() {
            $crate::testing::copy_is_a_second_track($make);
        }
        #[test]
        fn replaced_is_not_a_move() {
            $crate::testing::replaced_is_not_a_move($make);
        }
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
}
