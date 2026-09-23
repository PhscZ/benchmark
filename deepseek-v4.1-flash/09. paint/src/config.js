/** Central limits and defaults. Every value here is documented in README.md. */

/** Maximum supported document width/height in pixels. */
export const MAX_DIMENSION = 4096;

/** Maximum accepted source file size before decoding (defensive, not a feature limit). */
export const MAX_FILE_BYTES = 64 * 1024 * 1024;

/**
 * History memory budget. States are full-resolution RGBA snapshots, so one state
 * costs width * height * 4 bytes: 8.3 MiB at 1920x1080, 67.1 MiB at 4096x4096.
 * 256 MiB therefore holds 30 states (>= 20 undo steps) at 1920x1080 and 3 states
 * at 4096x4096, where older states are evicted with a visible notice.
 */
export const HISTORY_BUDGET_BYTES = 256 * 1024 * 1024;

/** Never evict below this many states, even if the budget cannot hold them. */
export const HISTORY_MIN_STATES = 2;

export const ZOOM_MIN = 0.05;
export const ZOOM_MAX = 32;
export const ZOOM_STEP = 1.25;

export const BRUSH_SIZE_MIN = 1;
export const BRUSH_SIZE_MAX = 200;
export const BRUSH_SIZE_DEFAULT = 12;

export const JPEG_QUALITY_DEFAULT = 0.92;

export const EXPORT_BACKGROUND_DEFAULT = '#ffffff';

export const RESIZE_MIN = 1;

export const DEFAULT_COLOR = '#ff3b30';

export const CHECKER_LIGHT = '#ffffff';
export const CHECKER_DARK = '#cfd3d8';
export const CHECKER_SIZE = 8;
