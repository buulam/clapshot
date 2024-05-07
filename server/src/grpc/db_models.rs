use lib_clapshot_grpc::proto;
use crate::database::error::{DBResult, DBError};

use super::{datetime_to_proto3, proto3_to_datetime};


pub fn proto_msg_type_to_event_name(t: proto::user_message::Type) -> &'static str {
    match t {
        proto::user_message::Type::Ok => "ok",
        proto::user_message::Type::Error => "error",
        proto::user_message::Type::Progress => "progress",
        proto::user_message::Type::VideoUpdated => "video_updated",
        proto::user_message::Type::VideoAdded => "video_added"
    }
}

pub fn msg_event_name_to_proto_msg_type(t: &str) -> proto::user_message::Type {
    match t {
        "ok" => proto::user_message::Type::Ok,
        "error" => proto::user_message::Type::Error,
        "progress" => proto::user_message::Type::Progress,
        "video_updated" => proto::user_message::Type::VideoUpdated,
        "video_added" => proto::user_message::Type::VideoAdded,
        _ => proto::user_message::Type::Ok,
    }
}


// ============================ Video ============================

impl crate::database::models::Video
{
    pub fn from_proto3(v: &proto::Video) -> DBResult<Self>
    {
        Ok(Self {
            id: v.id.clone(),
            user_id: v.added_by.as_ref().map(|u| u.id.clone()),
            user_name: v.added_by.as_ref().map(|u| u.name.clone()).unwrap_or_default(),
            added_time: v.added_time.as_ref().map(|t| proto3_to_datetime(t)).flatten().ok_or(DBError::Other(anyhow::anyhow!("Bad added_time")))?,
            recompression_done: v.processing_metadata.as_ref().map(|m| m.recompression_done.as_ref().map(|x| proto3_to_datetime(x))).flatten().flatten(),
            thumb_sheet_cols: v.preview_data.as_ref().map(|d| d.thumb_sheet.as_ref().map(|x| x.cols as i32)).flatten(),
            thumb_sheet_rows: v.preview_data.as_ref().map(|d| d.thumb_sheet.as_ref().map(|x| x.rows as i32)).flatten(),
            orig_filename: v.processing_metadata.as_ref().map(|m| m.orig_filename.clone()),
            title: v.title.clone(),
            total_frames: v.duration.as_ref().map(|d| d.total_frames as i32),
            duration: v.duration.as_ref().map(|d| d.duration as f32),
            fps: v.duration.as_ref().map(|d| d.fps.clone()),
            raw_metadata_all: v.processing_metadata.as_ref().map(|m| m.ffprobe_metadata_all.clone()).flatten(),
        })
    }

    pub fn to_proto3(&self, url_base: &str) -> proto::Video
    {
        let duration = match (self.duration, self.total_frames, &self.fps) {
            (Some(dur), Some(total_frames), Some(fps)) => Some(proto::VideoDuration {
                duration: dur as f64,
                total_frames: total_frames as i64,
                fps: fps.clone(),
            }),
            _ => None,
        };
        let added_by = match (&self.user_id, &self.user_name) {
            (Some(user_id), user_name) => Some(proto::UserInfo {
                id: user_id.clone(),
                name: user_name.clone(),
            }),
            _ => None,
        };
        let processing_metadata = match (&self.orig_filename, &self.recompression_done, &self.raw_metadata_all.clone()) {
            (Some(orig_filename), recompression_done, ffprobe_metadata_all) => Some(proto::VideoProcessingMetadata {
                orig_filename: orig_filename.clone(),
                recompression_done: recompression_done.map(|t| datetime_to_proto3(&t)),
                ffprobe_metadata_all: ffprobe_metadata_all.clone(),
            }),
            _ => None,
        };
        let preview_data = if let (Some(cols), Some(rows)) = (self.thumb_sheet_cols, self.thumb_sheet_rows) {
            let thumb_sheet = Some(proto::video_preview_data::ThumbSheet {
                url: format!("{}/videos/{}/thumbs/sheet-{}x{}.webp", url_base, &self.id, cols, rows),
                rows: rows as u32,
                cols: cols as u32,
            });
            Some(proto::VideoPreviewData {
                thumb_url: Some(format!("{}/videos/{}/thumbs/thumb.webp", url_base, &self.id)),
                thumb_sheet,
            }
            ) } else { None };

        // Use transcoded or orig video?
        let orig_uri = match &self.orig_filename {
            Some(f) => Some(format!("orig/{}", urlencoding::encode(f))),
            None => None
        };
        let playback_uri = match self.recompression_done {
            Some(_) => Some("video.mp4".into()),
            None => orig_uri.clone()
        };

        proto::Video {
            id: self.id.clone(),
            title: self.title.clone(),
            added_by,
            duration,
            added_time: Some(datetime_to_proto3(&self.added_time)),
            preview_data: preview_data,
            processing_metadata: processing_metadata,
            playback_url: playback_uri.map(|uri| format!("{}/videos/{}/{}", url_base, &self.id, uri)),
            orig_url: orig_uri.map(|uri| format!("{}/videos/{}/{}", url_base, &self.id, uri))
        }
    }
}

impl crate::database::models::VideoInsert
{
    pub fn from_proto3(v: &proto::Video) -> DBResult<Self>
    {
        Ok(Self {
            id: v.id.clone(),
            user_id: v.added_by.as_ref().map(|u| u.id.clone()),
            user_name: v.added_by.as_ref().map(|u| u.name.clone()).unwrap_or_default(),
            recompression_done: v.processing_metadata.as_ref().map(|m| m.recompression_done.as_ref().map(|x| proto3_to_datetime(x))).flatten().flatten(),
            thumb_sheet_cols: v.preview_data.as_ref().map(|d| d.thumb_sheet.as_ref().map(|x| x.cols as i32)).flatten(),
            thumb_sheet_rows: v.preview_data.as_ref().map(|d| d.thumb_sheet.as_ref().map(|x| x.rows as i32)).flatten(),
            orig_filename: v.processing_metadata.as_ref().map(|m| m.orig_filename.clone()),
            title: v.title.clone(),
            total_frames: v.duration.as_ref().map(|d| d.total_frames as i32),
            duration: v.duration.as_ref().map(|d| d.duration as f32),
            fps: v.duration.as_ref().map(|d| d.fps.clone()),
            raw_metadata_all: v.processing_metadata.as_ref().map(|m| m.ffprobe_metadata_all.clone()).flatten(),
        })
    }
}

// ============================ Comment ============================

impl crate::database::models::Comment
{
    pub fn from_proto3(v: &proto::Comment) -> DBResult<Self>
    {
        let user = v.user.as_ref().ok_or(anyhow::anyhow!("Missing user"))?;
        let created = v.created.as_ref().ok_or(anyhow::anyhow!("Missing created timestamp"))?;
        Ok(Self {
            id: v.id.parse().map_err(|_| DBError::Other(anyhow::anyhow!("Invalid comment ID")))?,
            video_id: v.video_id.clone(),
            user_id: user.id.clone(),
            user_name: user.name.clone().unwrap_or(user.id.clone()).clone(),
            comment: v.comment.clone(),
            timecode: v.timecode.clone(),
            parent_id: v.parent_id.as_ref().map(|id| id.parse()).transpose().map_err(|_| DBError::Other(anyhow::anyhow!("Invalid parent ID")))?,
            created: proto3_to_datetime(created).ok_or(anyhow::anyhow!("Invalid 'created' timestamp"))?,
            edited: v.edited.as_ref().map(|t| proto3_to_datetime(t)).flatten(),
            drawing: v.drawing.clone(),
        })
    }

    pub fn to_proto3(&self) -> proto::Comment
    {
        let user = proto::UserInfo {
            id: self.user_id.clone(),
            name: Some(self.user_name.clone()),
        };

        let created_timestamp = Some(datetime_to_proto3(&self.created));
        let edited_timestamp = self.edited.map(|edited| datetime_to_proto3(&edited));

        proto::Comment {
            id: self.id.to_string(),
            video_id: self.video_id.clone(),
            user: Some(user),
            comment: self.comment.clone(),
            timecode: self.timecode.clone(),
            parent_id: self.parent_id.map(|id| id.to_string()),
            created: created_timestamp,
            edited: edited_timestamp,
            drawing: self.drawing.clone(),
        }
    }
}

impl crate::database::models::CommentInsert
{
    pub fn from_proto3(v: &proto::Comment) -> DBResult<Self>
    {
        if v.id != String::default() {
            return Err(DBError::Other(anyhow::anyhow!("Comment ID must be empty for conversion to CommentInsert, which doesn't have 'id' field")));
        }
        let user = v.user.as_ref().ok_or(anyhow::anyhow!("Missing user"))?;
        Ok(Self {
            video_id: v.video_id.clone(),
            user_id: user.id.clone(),
            user_name: user.name.clone().unwrap_or(user.id.clone()).clone(),
            comment: v.comment.clone(),
            timecode: v.timecode.clone(),
            parent_id: v.parent_id.as_ref().map(|id| id.parse()).transpose().map_err(|_| DBError::Other(anyhow::anyhow!("Invalid parent ID")))?,
            drawing: v.drawing.clone(),
        })
    }
}

// ============================ Message ============================

impl crate::database::models::Message
{
    pub fn from_proto3(v: &proto::UserMessage) -> DBResult<Self>
    {
        let created = v.created.as_ref().ok_or(anyhow::anyhow!("Missing created timestamp"))?;
        let user_id = v.user_id.as_ref().ok_or(anyhow::anyhow!("Missing user"))?;
        let id = v.id.as_ref().ok_or(anyhow::anyhow!("Missing message ID"))?;
        Ok(Self {
            id: id.parse().map_err(|_| DBError::Other(anyhow::anyhow!("Invalid message ID")))?,
            event_name: proto_msg_type_to_event_name(v.r#type()).to_string(),
            user_id: user_id.clone(),
            video_id: v.refs.as_ref().map(|r| r.video_id.clone()).flatten(),
            comment_id: v.refs.as_ref().map(|r| r.comment_id.as_ref().map(|id| id.parse()).transpose().map_err(|_| DBError::Other(anyhow::anyhow!("Invalid comment ID")))).transpose()?.flatten(),
            message: v.message.clone(),
            details: v.details.clone().unwrap_or_default(),
            created: proto3_to_datetime(created).ok_or(anyhow::anyhow!("Invalid 'created' timestamp"))?,
            seen: v.seen,
        })
    }

    pub fn to_proto3(&self) -> proto::UserMessage
    {
        proto::UserMessage {
            id: Some(self.id.to_string()),
            r#type: msg_event_name_to_proto_msg_type(&self.event_name.as_str()).into(),
            user_id: Some(self.user_id.clone()),
            refs:Some(proto::user_message::Refs {
                video_id: self.video_id.clone(),
                comment_id: self.comment_id.map(|id| id.to_string()),
            }),
            message: self.message.clone(),
            details: if self.details.is_empty() { None } else { Some(self.details.clone()) },
            created: Some(datetime_to_proto3(&self.created)),
            seen: self.seen
        }
    }
}

impl crate::database::models::MessageInsert
{
    pub fn from_proto3(v: &proto::UserMessage) -> DBResult<Self>
    {
        if v.id.is_some() {
            return Err(DBError::Other(anyhow::anyhow!("Message ID must be empty for conversion to MessageInsert, which doesn't have 'id' field")));
        }
        let user_id = v.user_id.as_ref().ok_or(anyhow::anyhow!("Missing user"))?;

        Ok(Self {
            event_name: proto_msg_type_to_event_name(v.r#type()).to_string(),
            user_id: user_id.clone(),
            video_id: v.refs.as_ref().map(|r| r.video_id.clone()).flatten(),
            comment_id: v.refs.as_ref().map(|r| r.comment_id.as_ref().map(|id| id.parse()).transpose().map_err(|_| DBError::Other(anyhow::anyhow!("Invalid comment ID")))).transpose()?.flatten(),
            message: v.message.clone(),
            details: v.details.clone().unwrap_or_default(),
            seen: v.seen,
        })
    }

    pub fn to_proto3(&self) -> proto::UserMessage
    {
        proto::UserMessage {
            id: None,
            r#type: msg_event_name_to_proto_msg_type(&self.event_name.as_str()).into(),
            user_id: Some(self.user_id.clone()),
            refs:Some(proto::user_message::Refs {
                video_id: self.video_id.clone(),
                comment_id: self.comment_id.map(|id| id.to_string()),
            }),
            message: self.message.clone(),
            details: if self.details.is_empty() { None } else { Some(self.details.clone()) },
            created: None,
            seen: self.seen
        }
    }
}
