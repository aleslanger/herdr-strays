# Graph Report - /home/ales/projects/herdr-strays  (2026-07-31)

## Corpus Check
- Corpus is ~27,734 words - fits in a single context window. You may not need a graph.

## Summary
- 508 nodes · 1065 edges · 20 communities (18 shown, 2 thin omitted)
- Extraction: 93% EXTRACTED · 7% INFERRED · 0% AMBIGUOUS · INFERRED: 78 edges (avg confidence: 0.83)
- Token cost: 290,690 input · 0 output

## Community Hubs (Navigation)
- Git Status Parsing
- Worktree Integration Tests
- App State and Selection
- Safe Editor Launch
- Claude Agent Hand-off
- Tree Building and Folding
- Herdr Project Discovery
- Event Loop and Watcher
- Git Diff and Command Runner
- Demo Recording UI Concepts
- README and CI Concepts
- Help Overlay UI Concepts
- Main Screenshot UI Concepts
- Diff Scrolling
- Prompt Input Editing
- Show-All Mode UI Concepts
- Terminal Rendering (ratatui)
- Noise Suppression Rationale
- Open Shell Script

## God Nodes (most connected - your core abstractions)
1. `App` - 32 edges
2. `repo_with_commit()` - 31 edges
3. `parse_status()` - 23 edges
4. `flatten()` - 23 edges
5. `list_strays()` - 16 edges
6. `Stray` - 16 edges
7. `ProjectStrays` - 15 edges
8. `Row` - 15 edges
9. `git()` - 15 edges
10. `GitError` - 14 edges

## Surprising Connections (you probably didn't know these)
- `a_repository_under_two_workspaces_falls_back_to_its_directory_name()` --calls--> `projects_from()`  [INFERRED]
  tests/worktree_test.rs → src/discover.rs
- `a_workspace_label_names_its_project()` --calls--> `projects_from()`  [INFERRED]
  tests/worktree_test.rs → src/discover.rs
- `several_real_repositories_flatten_into_one_tree()` --calls--> `projects_from()`  [INFERRED]
  tests/worktree_test.rs → src/discover.rs
- `a_real_file_named_like_a_flag_reaches_the_editor_as_a_filename()` --calls--> `build_argv()`  [INFERRED]
  tests/worktree_test.rs → src/editor.rs
- `editor_argv_for_a_real_repo_path_keeps_the_file_separate()` --calls--> `build_argv()`  [INFERRED]
  tests/worktree_test.rs → src/editor.rs

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **Safety invariants: read-only git, shell-free editor launch, submodules never opened** — readme_read_only_git, readme_editor_launch_safety, readme_submodule_handling, readme_gitignore_honoured [INFERRED 0.85]
- **Stray-tree rendering: discovery, scope, status markers, folding, views** — readme_project_discovery, readme_workspace_scope, readme_diff_scope_head, readme_status_markers, readme_large_directory_folding, readme_show_all_view [INFERRED 0.85]
- **CI quality gates: format, clippy, cross-OS tests, dependency audit** — github_workflows_ci_check, github_workflows_ci_os_matrix, github_workflows_ci_audit, github_workflows_ci_audit_toml_policy [EXTRACTED 1.00]
- **Demo Walkthrough: navigate tree, read diff, toggle all tracked files, open keys reference** — docs_demo_strays_tree_pane, docs_demo_unified_diff_rendering, docs_demo_show_all_toggle, docs_demo_keys_overlay, docs_demo_fold_unfold_navigation [EXTRACTED 1.00]
- **Glanceable State Encoding: glyph markers, branch label, clean indicator, stray counts, dimming** — docs_demo_status_markers, docs_demo_branch_label, docs_demo_clean_project_indicator, docs_demo_pane_title_counter, docs_demo_dimmed_unchanged_files [INFERRED 0.85]
- **Three-Repository Demo Fixture (api-gateway, web-app, notes)** — docs_demo_demo_repo_api_gateway, docs_demo_demo_repo_web_app, docs_demo_demo_repo_notes, docs_demo_project_grouping [EXTRACTED 1.00]
- **Help Overlay Screen Composition (tree pane, keys pane, status bar)** — docs_help_stray_tree_pane, docs_help_keys_pane, docs_help_status_bar_hints, docs_help_two_pane_layout [EXTRACTED 1.00]
- **Stray Triage Flow: navigate, inspect diff, act** — docs_help_selection_cursor, docs_help_move_keybindings, docs_help_diff_preview_scrolling, docs_help_editor_env_handoff, docs_help_claude_prompt_handoff [INFERRED 0.85]
- **Visibility Scope Controls (filter, workspace scope, auto-follow)** — docs_help_stray_filter_toggle, docs_help_workspace_scope_toggle, docs_help_auto_follow_worktree, docs_help_stray_count_header [INFERRED 0.85]
- **Browse-Select-Preview Flow (tree cursor drives diff pane)** — docs_screenshot_project_tree_pane, docs_screenshot_selection_cursor, docs_screenshot_diff_preview_pane, docs_screenshot_action_move [INFERRED 0.85]
- **Stray Triage Action Set (edit, hand off to agent, filter scope)** — docs_screenshot_action_edit, docs_screenshot_action_claude, docs_screenshot_action_strays_filter, docs_screenshot_action_all_workspaces, docs_screenshot_stray_change_concept [INFERRED 0.75]
- **Repository State Visual Language (markers, branch labels, counts)** — docs_screenshot_git_status_markers, docs_screenshot_branch_and_clean_state_labels, docs_screenshot_multi_project_aggregation, docs_screenshot_foldable_tree_nodes [INFERRED 0.75]
- **Show-All Mode TUI Surface Composition** — docs_show_all_strays_pane_header, docs_show_all_multi_project_tree, docs_show_all_diff_pane, docs_show_all_status_bar, docs_show_all_cursor_indicator [EXTRACTED 1.00]
- **Example Projects Rendered in Show-All Mode** — docs_show_all_api_gateway_project, docs_show_all_notes_project, docs_show_all_web_app_project [EXTRACTED 1.00]

## Communities (20 total, 2 thin omitted)

### Community 0 - "Git Status Parsing"
Cohesion: 0.08
Nodes (41): a_renamed_submodule_is_still_a_submodule(), a_submodule_is_not_reported_as_a_modified_file(), an_ordinary_file_alongside_submodules_stays_modified(), branch_of(), field(), field_after(), find(), ignored_files_never_appear() (+33 more)

### Community 1 - "Worktree Integration Tests"
Cohesion: 0.09
Nodes (41): list_strays(), TempDir, a_detached_head_reports_its_commit_rather_than_the_word_head(), a_project_reports_the_branch_it_is_on(), a_real_file_named_like_a_flag_reaches_the_editor_as_a_filename(), a_real_submodule_is_reported_as_a_directory_not_an_editable_file(), a_repository_under_two_workspaces_falls_back_to_its_directory_name(), a_repository_with_no_commits_still_names_its_branch() (+33 more)

### Community 2 - "App State and Selection"
Cohesion: 0.14
Nodes (13): a_silent_refresh_keeps_the_reader_where_they_were(), App, load_project(), merge_tracked(), Notice, BTreeSet, Into, Option (+5 more)

### Community 3 - "Safe Editor Launch"
Cohesion: 0.11
Nodes (29): ExitStatus, OsString, a_path_that_looks_like_a_flag_is_separated_by_a_double_dash(), argv_strings(), blank_editor_setting_is_rejected(), build_argv(), deleted_stray_refuses_hand_off_instead_of_panicking(), editor_setting() (+21 more)

### Community 4 - "Claude Agent Hand-off"
Cohesion: 0.10
Nodes (28): a_carriage_return_cannot_overwrite_what_came_before(), a_newline_in_the_path_cannot_submit_on_the_users_behalf(), a_newline_typed_into_the_prompt_is_flattened_too(), a_prompt_that_is_only_control_characters_leaves_just_the_path(), Agent, an_ansi_escape_in_the_path_cannot_repaint_the_terminal(), c1_control_characters_are_stripped_as_well(), compose() (+20 more)

### Community 5 - "Tree Building and Folding"
Cohesion: 0.14
Nodes (30): a_clean_project_still_gets_a_row(), a_directory_count_includes_files_in_its_subdirectories(), a_failing_project_carries_its_error_onto_the_row(), a_folded_directory_reports_how_much_it_hides(), an_oversized_directory_starts_folded(), auto_folded(), collapsed_ancestor(), collapsible_rows_map_to_stable_node_ids() (+22 more)

### Community 6 - "Herdr Project Discovery"
Cohesion: 0.14
Nodes (26): BTreeMap, a_brace_inside_a_path_does_not_split_a_record(), a_path_containing_a_quote_is_unescaped(), nested_objects_inside_a_pane_are_not_mistaken_for_records(), open_projects(), pane_cwds(), PaneCwd, panes_sharing_a_repository_collapse_into_one_project() (+18 more)

### Community 7 - "Event Loop and Watcher"
Cohesion: 0.11
Nodes (23): Duration, Event, KeyEvent, Receiver, RecommendedWatcher, event_loop(), handle_key(), handle_prompt_key() (+15 more)

### Community 8 - "Git Diff and Command Runner"
Cohesion: 0.11
Nodes (30): I, diff_for(), diff_tracked(), diff_untracked(), looks_binary(), Path, Result, GitError (+22 more)

### Community 9 - "Demo Recording UI Concepts"
Cohesion: 0.11
Nodes (27): Branch Label Beside Project Name, Claude Prompt Hand-off (c key), Clean Project Indicator ("clean" instead of a count), Demo Repository: api-gateway (feature/token-expiry), Demo Repository: notes (main, clean), Demo Repository: web-app (main), Diff Pane, Empty-Diff Placeholder Hint ("Select a file to see its diff.") (+19 more)

### Community 10 - "README and CI Concepts"
Cohesion: 0.12
Nodes (24): audit job (rustsec/audit-check), Shared advisory policy in .cargo/audit.toml, check job (fmt, clippy, test), CI workflow, Give git an identity step, ubuntu/macos OS matrix, Branch display with detached-HEAD fallback, Claude agent hand-off (+16 more)

### Community 11 - "Help Overlay UI Concepts"
Cohesion: 0.12
Nodes (22): Act Keybindings (e edit, c claude, r refresh), ASCII Box-Drawing Pane Chrome, List Follows the Worktree Automatically, Write a Prompt About It for Claude, Clean Project Row (notes main clean), Diff Preview Scrolling, Open File in $EDITOR Handoff, Strays Help Overlay Screenshot (+14 more)

### Community 12 - "Main Screenshot UI Concepts"
Cohesion: 0.13
Nodes (22): Action: w all ws (all workspaces), Action: c claude (agent hand-off), Action: e edit, Action: fold, Action: ? keys (help overlay), Action: j/k move, Action: a strays (toggle strays-only view), Branch and Clean-State Labels (+14 more)

### Community 13 - "Diff Scrolling"
Cohesion: 0.25
Nodes (9): a_diff_shorter_than_the_pane_does_not_scroll(), App, app_with_diff(), home_returns_to_the_top(), jumping_to_the_end_lands_at_the_last_screenful(), paging_down_moves_by_almost_a_screen(), Self, scrolling_cannot_pass_the_end_of_the_diff() (+1 more)

### Community 14 - "Prompt Input Editing"
Cohesion: 0.26
Nodes (9): an_open_prompt_captures_typing(), App, backspacing_an_empty_prompt_leaves_it_open(), empty_app(), Option, Self, String, sending_with_nothing_selected_closes_the_prompt() (+1 more)

### Community 15 - "Show-All Mode UI Concepts"
Cohesion: 0.16
Nodes (14): api-gateway Example Project (feature/token-expiry), Cursor Indicator (> gutter marker), Diff Pane with Placeholder Hints, Collapsible Directory Folding, Multi-Project Aggregated File Tree, notes Example Project (main), Project Row (name, branch, stray count), strays Show-All Mode TUI Screenshot (+6 more)

### Community 16 - "Terminal Rendering (ratatui)"
Cohesion: 0.48
Nodes (11): Frame, ListItem, Rect, draw(), draw_diff(), draw_help(), draw_nothing_found(), draw_status() (+3 more)

## Knowledge Gaps
- **17 isolated node(s):** `open.sh script`, `NulRecords`, `Honours .gitignore`, `Branch display with detached-HEAD fallback`, `Branch Label Beside Project Name` (+12 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **2 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Stray` connect `Git Status Parsing` to `Worktree Integration Tests`, `App State and Selection`, `Safe Editor Launch`, `Tree Building and Folding`, `Git Diff and Command Runner`?**
  _High betweenness centrality (0.058) - this node is a cross-community bridge._
- **Why does `App` connect `App State and Selection` to `Git Diff and Command Runner`, `Tree Building and Folding`, `Herdr Project Discovery`?**
  _High betweenness centrality (0.044) - this node is a cross-community bridge._
- **Why does `list_strays()` connect `Worktree Integration Tests` to `Git Status Parsing`, `Git Diff and Command Runner`, `App State and Selection`?**
  _High betweenness centrality (0.026) - this node is a cross-community bridge._
- **Are the 2 inferred relationships involving `flatten()` (e.g. with `.rebuilt()` and `several_real_repositories_flatten_into_one_tree()`) actually correct?**
  _`flatten()` has 2 INFERRED edges - model-reasoned connections that need verification._
- **What connects `open.sh script`, `NulRecords`, `Honours .gitignore` to the rest of the system?**
  _17 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Git Status Parsing` be split into smaller, more focused modules?**
  _Cohesion score 0.0803633822501747 - nodes in this community are weakly interconnected._
- **Should `Worktree Integration Tests` be split into smaller, more focused modules?**
  _Cohesion score 0.09250693802035152 - nodes in this community are weakly interconnected._