#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
type WsMsg = warp::ws::Message;

type Res<T> = anyhow::Result<T>;
type MsgSender = tokio::sync::mpsc::UnboundedSender<WsMsg>;
type SenderList = Vec<MsgSender>;
type SenderListMap = Arc<RwLock<HashMap<String, SenderList>>>;

use serde_json::json;
use anyhow::{anyhow, bail, Context};

use data_url::{DataUrl, mime};
use sha2::{Sha256, Digest};
use hex;

use super::WsSessionArgs;

use crate::api_server::server_state::ServerState;
use crate::database::error::DBError;
use crate::database::{models, DB};
use crate::database::schema::comments::drawing;


// ---------------------------------------------------------------------
// Result sending helpers
// ---------------------------------------------------------------------

enum Topic<'a> {
    Video(&'a str),
    Comment(i32),
    None
}

macro_rules! send_user_msg(
    ($event_name:expr, $ses:expr, $topic:expr, $msg:expr, $details:expr, $persist:expr) => {
        let (comment_id, video_hash) = match $topic {
            Topic::Video(video_hash) => (None, Some(video_hash.into())),
            Topic::Comment(comment_id) => (Some(comment_id.into()), None),
            Topic::None => (None, None)
        };
        $ses.push_notify_message(&models::MessageInsert {
            event_name: $event_name.into(),
            user_id: $ses.user_id.into(),
            ref_comment_id: comment_id,
            seen: false,
            ref_video_hash: video_hash,
            message: $msg.into(),
            details: $details.into()
        }, $persist)?;
    };
    ($event_name:expr, $ses:expr, $topic:expr, $msg:expr, $persist:expr) => {
        send_user_error!($ses, $topic, $msg, String::new(), $persist)
    };
    ($event_name:expr, $ses:expr, $topic:expr, $msg:expr) => {
        send_user_error!($ses, $topic, $msg, String::new(), false)
    };
);

macro_rules! send_user_error(
    ($ses:expr, $topic:expr, $msg:expr, $details:expr, $persist:expr) => { send_user_msg!("error", $ses, $topic, $msg, $details, $persist); };
    ($ses:expr, $topic:expr, $msg:expr, $persist:expr) => { send_user_error!($ses, $topic, $msg, String::new(), $persist); };
    ($ses:expr, $topic:expr, $msg:expr) => { send_user_error!($ses, $topic, $msg, String::new(), false); };
);

macro_rules! send_user_ok(
    ($ses:expr, $topic:expr, $msg:expr, $details:expr, $persist:expr) => { send_user_msg!("ok", $ses, $topic, $msg, $details, $persist); };
    ($ses:expr, $topic:expr, $msg:expr, $persist:expr) => { send_user_ok!($ses, $topic, $msg, String::new(), $persist); };
    ($ses:expr, $topic:expr, $msg:expr) => { send_user_ok!($ses, $topic, $msg, String::new(), false); };
);

// ---------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------

/// Send user a list of all videos they have.
pub async fn msg_list_my_videos(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let mut videos = ses.server.db.get_all_user_videos(&ses.user_id)?;
    videos.sort_by_cached_key(|v| -v.added_time.timestamp_millis());    // newest first
    let videos = videos.into_iter().map(|v| {
            let mut fields = v.to_json()?;
            if let Some(sheet_dims) = v.thumb_sheet_dims {
                let (sheet_w, sheet_h) = sheet_dims.split_once('x').ok_or(anyhow!("Invalid sheet dims"))?;
                fields["thumb_sheet_cols"] = json!(sheet_w.parse::<u32>()?);
                fields["thumb_sheet_rows"] = json!(sheet_h.parse::<u32>()?);
                fields["thumb_url"] = json!(format!("{}/videos/{}/thumbs/thumb.webp", ses.server.url_base, &v.video_hash));
                fields["thumb_sheet_url"] = json!(format!("{}/videos/{}/thumbs/sheet-{}.webp", ses.server.url_base, &v.video_hash, sheet_dims));
            };
            Ok(fields)
        }).collect::<Res<Vec<serde_json::Value>>>()?;


    ses.emit_cmd("user_videos", &json!({
            "username": ses.user_name.clone(),
            "user_id": ses.user_id.clone(),
            "videos": videos }),
        super::SendTo::CurSession())?;
    Ok(())
}

/// User opens a video.
/// Send them the video info and all comments related to it.
/// Register the session as a viewer of the video (video_session_guard).
pub async fn msg_open_video(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let video_hash = data["video_hash"].as_str().ok_or(anyhow!("video_hash missing"))?;
    match ses.server.db.get_video(video_hash) {
        Err(DBError::NotFound()) => {
            send_user_error!(ses, Topic::Video(video_hash), "No such video.");
        }
        Err(e) => { bail!(e); }
        Ok(v) => {
            ses.video_session_guard = Some(ses.server.link_session_to_video(video_hash, ses.sender.clone()));
            let mut fields = v.to_json()?;

            // Use transcoded or orig video?
            let (file, uri) = match v.recompression_done {
                Some(_) => Ok(("video.mp4".into(), "video.mp4".into())),
                None => match &v.orig_filename {
                    Some(f) => Ok((format!("orig/{}", f), format!("orig/{}", urlencoding::encode(f)))),
                    None => Err(anyhow!("No video file"))
                }}?;

            fields["video_url"] = json!(format!("{}/videos/{}/{}", ses.server.url_base, &v.video_hash, uri));
            ses.emit_cmd("open_video", &fields, super::SendTo::CurSession() )?;

            for c in ses.server.db.get_video_comments(video_hash)? {
                let cid = c.id;
                if let Err(e) = ses.emit_new_comment(c, super::SendTo::CurSession()).await {
                    tracing::error!("Error sending comment: {}", e);
                    send_user_error!(ses, Topic::Comment(cid), format!("Error sending comment #{cid}: {:?}", e));
                }
            }
        }
    }
    ses.cur_video_hash = Some(video_hash.into());
    Ok(())
}

pub async fn msg_del_video(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let video_hash = data["video_hash"].as_str().ok_or(anyhow!("video_hash missing"))?;
    match ses.server.db.get_video(video_hash) {
        Ok(v) => {
            if Some(ses.user_id.to_string()) != v.added_by_userid && ses.user_id != "admin" {
                send_user_error!(ses, Topic::Video(video_hash), "Video not owned by you. Cannot delete.");
            } else {
                ses.server.db.del_video_and_comments(video_hash)?;
                let mut details = format!("Added by {:?} ({:?}) on {}. Filename was {:?}.",
                    v.added_by_username, v.added_by_userid, v.added_time, v.orig_filename);

                fn backup_video_db_row(server: &ServerState, v: &models::Video) -> Res<()> {
                    let backup_file = server.videos_dir.join(v.video_hash.clone()).join("db_backup.json");
                    if backup_file.exists() {
                        std::fs::remove_file(&backup_file)?;
                    }
                    let json_str = serde_json::to_string_pretty(&v)?;
                    std::fs::write(&backup_file, json_str)?;
                    Ok(())
                }

                fn move_video_to_trash(server: &ServerState, video_hash: &str) -> Res<()>
                {
                    let video_dir = server.videos_dir.join(video_hash);
                    let trash_dir = server.videos_dir.join("trash");
                    if !trash_dir.exists() {
                        std::fs::create_dir(&trash_dir)?;
                    }
                    let hash_and_datetime = format!("{}_{}", video_hash, chrono::Utc::now().format("%Y%m%d-%H%M%S"));
                    let video_trash_dir = trash_dir.join(hash_and_datetime);
                    std::fs::rename(&video_dir, &video_trash_dir)?;
                    Ok(())
                }

                let mut cleanup_errors = false;
                if let Err(e) = backup_video_db_row(&ses.server, &v) {
                    details.push_str(&format!(" WARNING: DB row backup failed: {:?}.", e));
                    cleanup_errors = true;

                }
                if let Err(e) = move_video_to_trash(&ses.server, video_hash) {
                    details.push_str(&format!(" WARNING: Move to trash failed: {:?}.", e));
                    cleanup_errors = true;
                }

                send_user_ok!(ses, Topic::Video(video_hash),
                    if !cleanup_errors {"Video deleted."} else {"Video deleted, but cleanup had errors."},
                    details, true);
            }
        }
        Err(DBError::NotFound()) => {
            send_user_error!(ses, Topic::Video(video_hash), "No such video. Cannot delete.");
        }
        Err(e) => { bail!(e); }
    }
    Ok(())
}

pub async fn msg_rename_video(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let video_hash = data["video_hash"].as_str().ok_or(anyhow!("video_hash missing"))?;
    let new_name = data["new_name"].as_str().ok_or(anyhow!("new_name missing"))?;

    match ses.server.db.get_video(video_hash) {
        Ok(v) => {
            if Some(ses.user_id.to_string()) != v.added_by_userid && ses.user_id != "admin" {
                send_user_error!(ses, Topic::Video(video_hash), "Video not owned by you. Cannot rename.");
            } else {
                let new_name = new_name.trim();
                if new_name.is_empty() || !new_name.chars().any(|c| c.is_alphanumeric()) {
                    send_user_error!(ses, Topic::Video(video_hash), "Invalid video name (must have letters/numbers)");
                    return Ok(());
                }
                if new_name.len() > 160 {
                    send_user_error!(ses, Topic::Video(video_hash), "Video name too long (max 160)");
                    return Ok(());
                }
                ses.server.db.rename_video(video_hash, new_name)?;
                send_user_ok!(ses, Topic::Video(video_hash), "Video renamed.",
                    format!("New name: '{}'", new_name), true);
            }
        }
        Err(DBError::NotFound()) => {
            send_user_error!(ses, Topic::Video(video_hash), "No such video. Cannot rename.");
        }
        Err(e) => { bail!(e); }
    }
    Ok(())
}

pub async fn msg_add_comment(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let vh = data["video_hash"].as_str().ok_or(anyhow!("video_hash missing"))?;

    if let Err(DBError::NotFound())  = ses.server.db.get_video(&vh) {
        send_user_error!(ses, Topic::Video(&vh), "No such video. Cannot comment.");
        return Ok(());
    }

    // Parse drawing data if present and write to file
    let mut drwn = data["drawing"].as_str().map(|s| s.to_string());
    if let Some(d) = drwn.clone() {
        if d.starts_with("data:") {

            // Convert data URI to bytes
            let img_uri = DataUrl::process(&d).map_err(|e| anyhow!("Invalid drawing data URI"))?;

            if img_uri.mime_type().type_ != "image" || img_uri.mime_type().subtype != "webp" {
                bail!("Invalid mimetype in drawing: {:?}", img_uri.mime_type())
            }
            let img_data = img_uri.decode_to_vec().map_err(|e| anyhow!("Failed to decode drawing data URI: {:?}", e))?;

            // Make up a filename
            fn sha256hex( data: &[u8] ) -> String {
                let mut hasher = Sha256::new();
                hasher.update(data);
                let result = hasher.finalize();
                hex::encode(result)
            }
            let short_csum = sha256hex(img_data.0.as_ref())[..16].to_string();
            let fname = format!("{}.webp", short_csum);

            // Write to file
            let drawing_path = ses.server.videos_dir.join(&vh).join("drawings").join(&fname);
            std::fs::create_dir_all(drawing_path.parent().unwrap())
                .map_err(|e| anyhow!("Failed to create drawings dir: {:?}", e))?;
            async_std::fs::write(drawing_path, img_data.0).await.map_err(
                |e| anyhow!("Failed to write drawing file: {:?}", e))?;

            // Replace data URI with filename
            drwn = Some(fname);
        }
    };

    let c = models::CommentInsert {
        video_hash: vh.to_string(),
        parent_id: data["parent_id"].as_i64().map(|x| x as i32),
        user_id: ses.user_id.into(),
        username: ses.user_name.into(),
        comment: data["comment"].as_str().ok_or(anyhow!("comment missing"))?.to_string(),
        timecode: data["timecode"].as_str().map(String::from),
        drawing: drwn,
    };
    let new_id = ses.server.db.add_comment(&c)
        .map_err(|e| anyhow!("Failed to add comment: {:?}", e))?;
    let c = ses.server.db.get_comment(new_id)?;

    // Send to all clients watching this video
    ses.emit_new_comment(c, super::SendTo::VideoHash(&vh)).await?;
    Ok(())
}

pub async fn msg_edit_comment(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let comment_id = data["comment_id"].as_i64().ok_or(anyhow!("comment_id missing"))? as i32;
    let new_text = data["comment"].as_str().ok_or(anyhow!("comment missing"))?.to_string();

    match ses.server.db.get_comment(comment_id) {
        Ok(old) => {
            let vh = old.video_hash;
            if ses.user_id != old.user_id && ses.user_id != "admin" {
                send_user_error!(ses, Topic::Video(&vh), "Failed to edit comment.", "You can only edit your own comments", true);
                return Ok(());
            }
            ses.server.db.edit_comment(comment_id, &new_text)?;
            ses.emit_cmd("del_comment", &json!({ "comment_id": comment_id }), super::SendTo::VideoHash(&vh))?;
            let c = ses.server.db.get_comment(comment_id)?;
            ses.emit_new_comment(c, super::SendTo::VideoHash(&vh)).await?;
        }
        Err(DBError::NotFound()) => {
            send_user_error!(ses, Topic::None, "Failed to edit comment.", "No such comment. Cannot edit.", true);
        }
        Err(e) => { bail!(e); }
    }
    Ok(())
}

pub async fn msg_del_comment(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let comment_id = data["comment_id"].as_i64().ok_or(anyhow!("comment_id missing"))? as i32;

    match ses.server.db.get_comment(comment_id) {
        Ok(cmt) => {
            let vh = cmt.video_hash;
            if ses.user_id != cmt.user_id && ses.user_id != "admin" {
                send_user_error!(ses, Topic::Video(&vh), "Failed to delete comment.", "You can only delete your own comments", true);
                return Ok(());
            }
            let all_comm = ses.server.db.get_video_comments(&vh)?;
            if all_comm.iter().any(|c| c.parent_id == Some(comment_id)) {
                send_user_error!(ses, Topic::Video(&vh), "Failed to delete comment.", "Comment has replies. Cannot delete.", true);
                return Ok(());
            }
            ses.server.db.del_comment(comment_id)?;
            ses.emit_cmd("del_comment", &json!({ "comment_id": comment_id }), super::SendTo::VideoHash(&vh))?;
        }
        Err(DBError::NotFound()) => {
            send_user_error!(ses, Topic::None, "Failed to delete comment.", "No such comment. Cannot delete.", true);
        }
        Err(e) => { bail!(e); }
    }
    Ok(())
}

pub async fn msg_list_my_messages(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let msgs = ses.server.db.get_user_messages(&ses.user_id)?;
    for m in msgs {
        ses.emit_cmd("message", &m.to_json()?, super::SendTo::CurSession())?;
        if !m.seen {
            ses.server.db.set_message_seen(m.id, true)?;
        }
    }
    Ok(())
}

pub async fn msg_join_collab(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let collab_id = data["collab_id"].as_str().ok_or(anyhow!("collab_id missing"))?;
    let video_hash = data["video_hash"].as_str().ok_or(anyhow!("video_hash missing"))?;

    if let Some(collab_id) = ses.cur_collab_id.clone() {
        if ses.server.sender_is_collab_participant(collab_id.as_str(), &ses.sender) {
            tracing::debug!("{} is already in collab {}. Ignoring double join.", ses.user_name, collab_id);
            return Ok(());
        }
    }

    ses.collab_session_guard = None;
    ses.cur_collab_id = None;

    match ses.server.db.get_video(video_hash) {
        Err(DBError::NotFound()) => {
            send_user_error!(ses, Topic::Video(video_hash), "No such video.");
        }
        Err(e) => { bail!(e); }
        Ok(v) => {
            match ses.server.link_session_to_collab(collab_id, video_hash, ses.sender.clone()) {
                Ok(csg) => {
                    ses.collab_session_guard = Some(csg);
                    ses.cur_collab_id = Some(collab_id.to_string());
                    ses.emit_cmd("message", &json!({"event_name": "ok", "message": format!("'{}' joined collab", ses.user_name)}), super::SendTo::CurCollab())?;
                }
                Err(e) => {
                    send_user_error!(ses, Topic::Video(video_hash), format!("Failed to join collab session: {}", e));
                }
            }
        }
    }
    Ok(())
}

pub async fn msg_leave_collab(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    if ses.cur_collab_id.is_some() {
        ses.emit_cmd("message", &json!({"event_name": "ok", "message": format!("'{}' left collab", ses.user_name)}), super::SendTo::CurCollab())?;
        ses.collab_session_guard = None;
        ses.cur_collab_id = None;
    }
    Ok(())
}

pub async fn msg_collab_report(data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    if ses.cur_collab_id.is_none() {
        send_user_error!(ses, Topic::None, "Report rejected: no active collab session.");
        return Ok(());
    }
    let paused = data["paused"].as_bool().ok_or(anyhow!("paused missing"))?;
    let seek_time = data["seek_time"].as_f64().ok_or(anyhow!("seek_time missing"))?;
    let img_url = data["drawing"].as_str();
    let msg = if img_url.is_some() {
        json!({ "paused": paused, "seek_time": seek_time, "drawing": img_url, "from_user": &ses.user_name })
    } else {
        json!({ "paused": paused, "seek_time": seek_time, "from_user": &ses.user_name })
    };
    ses.emit_cmd("collab_cmd", &msg, super::SendTo::CurCollab()).map(|_| ())
}

// custom logout error with thiserror

#[derive(thiserror::Error, Debug)]
pub enum SessionClose {
    #[error("User logout")]
    Logout,
}


pub async fn msg_dispatch(cmd: &str, data: &serde_json::Value, ses: &mut WsSessionArgs<'_>) -> Res<()> {
    let res = match cmd {
        "list_my_videos" => msg_list_my_videos(data, ses).await,
        "open_video" => msg_open_video(data, ses).await,
        "del_video" => msg_del_video(data, ses).await,
        "rename_video" => msg_rename_video(data, ses).await,
        "add_comment" => msg_add_comment(data, ses).await,
        "edit_comment" => msg_edit_comment(data, ses).await,
        "del_comment" => msg_del_comment(data, ses).await,
        "list_my_messages" => msg_list_my_messages(data, ses).await,
        "join_collab" => msg_join_collab(data, ses).await,
        "leave_collab" => msg_leave_collab(data, ses).await,
        "collab_report" => msg_collab_report(data, ses).await,
        "logout" => {
            tracing::info!("logout from client: user={}", ses.user_id);
            return Err(SessionClose::Logout.into());
        },
        "echo" => {
            let answ = format!("Echo: {}", data.as_str().ok_or(anyhow!("data not found"))?);
            ses.sender.send(WsMsg::text(answ))?;
            Ok(())
        },
        _ => {
            send_user_error!(ses, Topic::None, format!("Unknown command: '{}'", cmd));
            Ok(())
        }
    };
    if let Err(e) = res {
        tracing::warn!("[{}] '{cmd}' failed: {}", ses.sid, e);
        send_user_error!(ses, Topic::None, format!("{cmd} failed: {e}"));
    }
    Ok(())
}
