export interface ClapshotVideoJson
{
    video_hash: string;
    added_by_userid: string | null;
    added_by_username: string | null;

    added_time: number; // unix timestamp

    recompression_done: string | null;
    orig_filename: string | null;
    total_frames: number | null;
    duration: number | null;
    fps: string | null;
    raw_metadata_all: string | null;

    title: string | null;

    thumb_url: string | null;
    thumb_sheet_url: string | null;
    thumb_sheet_cols: number | null;
    thumb_sheet_rows: number | null;
}

export interface ClapshotCommentJson
{
    id: number;
    video_hash: string;
    parent_id: number | null;

    created: number;  // unix timestamp
    edited: number | null;

    user_id: string;
    username: string;
    comment: string;
    timecode: string | null;
    drawing: string | null;
}
