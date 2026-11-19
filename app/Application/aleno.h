// C ABI exported by crates/aleno-client (libaleno_client.a). Keep in sync with src/ffi.rs.
#ifndef ALENO_H
#define ALENO_H
#include <stdint.h>
#include <stddef.h>

typedef struct AlenoCtl AlenoCtl;
typedef struct AlenoPane AlenoPane;

// Mirrors aleno_terminal::Cell (16 bytes).
typedef struct {
    uint32_t ch;      // scalar value, or CLUSTER_FLAG|index for grapheme clusters
    uint32_t fg;      // 0 default | 0x01000000|palette | 0x02000000|rgb
    uint32_t bg;
    uint16_t flags;
    uint16_t link;
} AlenoCell;

// Mirrors aleno_terminal::ViewInfo (64 bytes).
typedef struct {
    uint32_t cols, rows;
    uint32_t cursor_row, cursor_col, cursor_style, cursor_visible;
    uint32_t display_offset, scrollback_len, top_abs, modes;
    uint32_t needs_realloc, _pad;
    uint64_t evicted, bell_count;
} AlenoViewInfo;

// Mirrors aleno_terminal::BlockInfo (24 bytes): one OSC 133 command block. Rows are line indexes, -1 = unknown.
typedef struct {
    uint32_t prompt_row;
    int32_t output_row, end_row;
    int32_t exit_code;      // INT32_MIN when unknown
    int32_t dur_ms;         // -1 when unknown
    uint32_t running;
} AlenoBlock;

// Mirrors aleno_terminal::FinishedInfo (16 bytes): a command that just finished.
typedef struct {
    int32_t exit_code;      // INT32_MIN when unknown
    int32_t dur_ms;
    int32_t cmd_row;        // line index holding the command text, -1 if unknown
    int32_t _pad;
} AlenoFinished;

typedef void (*AlenoNotify)(void *userdata, uint32_t event);
enum { ALENO_EVENT_OUTPUT = 0, ALENO_EVENT_TITLE = 1, ALENO_EVENT_EXITED = 2, ALENO_EVENT_BELL = 3, ALENO_EVENT_DISCONNECTED = 4, ALENO_EVENT_COMMAND_FINISHED = 5, ALENO_EVENT_PROGRESS = 6 };

void aleno_string_free(char *p);
char *aleno_socket_path(void);

AlenoCtl *aleno_ctl_connect(const char *path);
void aleno_ctl_free(AlenoCtl *c);
int64_t aleno_ctl_create_session(const AlenoCtl *c, const char *name);
int32_t aleno_ctl_rename_session(const AlenoCtl *c, uint32_t session, const char *name);
int32_t aleno_ctl_kill_session(const AlenoCtl *c, uint32_t session);
char *aleno_ctl_list_sessions_json(const AlenoCtl *c);
char *aleno_ctl_list_panes_json(const AlenoCtl *c, uint32_t session);
int32_t aleno_ctl_set_layout(const AlenoCtl *c, uint32_t session, const char *json);
char *aleno_ctl_get_layout(const AlenoCtl *c, uint32_t session);
int64_t aleno_ctl_create_pane(const AlenoCtl *c, uint32_t session, uint16_t cols, uint16_t rows,
                              const char *shell, const char *cwd, uint32_t scrollback);
int64_t aleno_ctl_create_pane_seeded(const AlenoCtl *c, uint32_t session, uint16_t cols, uint16_t rows,
                                     const char *shell, const char *cwd, uint32_t scrollback, const char *seed, int32_t welcome);
int32_t aleno_ctl_kill_pane(const AlenoCtl *c, uint32_t session, uint32_t pane);

AlenoPane *aleno_pane_attach(const char *path, uint32_t session, uint32_t pane, uint16_t cols, uint16_t rows,
                             uint32_t scrollback, AlenoNotify notify, void *userdata);
void aleno_pane_free(AlenoPane *p);
void aleno_pane_input(const AlenoPane *p, const uint8_t *data, size_t len);
void aleno_pane_resize(const AlenoPane *p, uint16_t cols, uint16_t rows);
void aleno_pane_copy_view(const AlenoPane *p, uint32_t expect_cols, uint32_t expect_rows,
                          AlenoCell *cells, size_t cells_cap, uint8_t *dirty, size_t dirty_cap,
                          AlenoViewInfo *info);
void aleno_pane_scroll(const AlenoPane *p, int32_t delta);
void aleno_pane_set_display_offset(const AlenoPane *p, uint32_t off);
void aleno_pane_set_scrollback_limit(const AlenoPane *p, uint32_t n);
char *aleno_pane_text(const AlenoPane *p, uint32_t r0, uint32_t c0, uint32_t r1, uint32_t c1);
char *aleno_pane_cluster(const AlenoPane *p, uint32_t id);
char *aleno_pane_link(const AlenoPane *p, uint16_t id);
char *aleno_pane_title(const AlenoPane *p);
char *aleno_pane_cwd(const AlenoPane *p);
int32_t aleno_pane_is_exited(const AlenoPane *p);
int32_t aleno_pane_exit_code(const AlenoPane *p);
int32_t aleno_pane_is_disconnected(const AlenoPane *p);
size_t aleno_pane_search(const AlenoPane *p, const char *query, int32_t case_insensitive, uint32_t *out, size_t cap);
char *aleno_pane_snapshot(const AlenoPane *p, uint32_t max_history);   // owned; replay with aleno_ctl_create_pane_seeded
char *aleno_pane_block_command(const AlenoPane *p, uint32_t prompt_row);   // owned; NULL when unknown
void aleno_pane_progress(const AlenoPane *p, uint32_t *state, uint32_t *percent);   // OSC 9;4: 0 none, 1 normal, 2 error, 3 indeterminate, 4 paused
size_t aleno_pane_blocks(const AlenoPane *p, uint32_t r0, uint32_t r1, AlenoBlock *out, size_t cap);
int64_t aleno_pane_prompt_nav(const AlenoPane *p, uint32_t from, int32_t dir);
int32_t aleno_pane_last_block(const AlenoPane *p, int32_t finished_only, AlenoBlock *out);
size_t aleno_pane_take_finished(const AlenoPane *p, AlenoFinished *out, size_t cap);
size_t aleno_cell_size(void);
size_t aleno_view_info_size(void);
// Owned string; release with aleno_string_free.
char *aleno_ctl_last_error(void);
#endif
