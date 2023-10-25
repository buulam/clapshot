use std::collections::HashMap;
use lib_clapshot_grpc::proto::org::UserSessionData;

use lib_clapshot_grpc::proto::{self, org};

use crate::folder_ops::{FolderData, get_current_folder_path, fetch_folder_contents};
use crate::{GrpcServerConn, RpcResult};


#[derive(serde::Serialize, serde::Deserialize)]
pub struct OpenFolderArgs { pub id: String }


/// Popup actions for when the user right-clicks on a listing background.
pub (crate) fn make_folder_list_popup_actions() -> HashMap<String, proto::ActionDef> {
    HashMap::from([
        ("new_folder".into(), make_new_folder_action()),
    ])
}

/// Popup actions for when the user right-clicks on a folder item.
fn make_new_folder_action() -> proto::ActionDef {
    proto::ActionDef  {
        ui_props: Some(proto::ActionUiProps {
            label: Some(format!("New folder")),
            icon: Some(proto::Icon {
                src: Some(proto::icon::Src::FaClass(proto::icon::FaClass {
                    classes: "fa fa-folder-plus".into(), color: None, })),
                ..Default::default()
            }),
            key_shortcut: None,
            natural_desc: Some(format!("Create a new folder")),
            ..Default::default()
        }),
        action: Some(proto::ScriptCall {
            lang: proto::script_call::Lang::Javascript.into(),
            code: r#"
var folder_name = (await prompt("Name for the new folder", ""))?.trim();
if (folder_name) {
    await call_organizer("new_folder", {name: folder_name});
}
                "#.into()
        })
    }
}

// ---------------------------------------------------------------------------

/// Build folder view page.
/// Reads folder_path cookie and builds a list of folders and videos in the folder.
pub async fn construct_navi_page(srv: &mut GrpcServerConn, ses: &UserSessionData, cookie_override: Option<String>)
    -> RpcResult<org::ClientShowPageRequest>
{
    let folder_path = get_current_folder_path(srv, &ses, cookie_override).await?;
    assert!(!folder_path.is_empty(), "Folder path is empty, should always contain at least the root folder");
    let cur_folder = folder_path.last().unwrap();

    let (folders, videos) = fetch_folder_contents(srv, &cur_folder).await?;

    // Convert folder and video nodes to page items
    let folder_page_items = folders.into_iter().map(|f| folder_node_to_page_item(&f)).collect::<Vec<_>>();
    let video_page_items: Vec<proto::page_item::folder_listing::Item> = videos.iter().map(|v| {
        proto::page_item::folder_listing::Item {
            item: Some(proto::page_item::folder_listing::item::Item::Video(v.clone())),
            open_action: Some(proto::ScriptCall {
                lang: proto::script_call::Lang::Javascript.into(),
                code: r#"await call_server("open_video", {id: items[0].video.id});"#.into()
            }),
            popup_actions: vec!["popup_rename".into(), "popup_trash".into()],
            vis: None,
        }
    }).collect();
    let items = folder_page_items.into_iter().chain(video_page_items.into_iter()).collect();  // Concatenate folders + video

    let folder_listing = proto::page_item::FolderListing {
        items,
        allow_reordering: true,
        popup_actions: vec!["new_folder".into()],
        listing_id: cur_folder.id.clone() };
    let breadcrumbs_html = make_bredcrumbs_html(folder_path);

    Ok(org::ClientShowPageRequest {
            sid: ses.sid.clone(),
            page_items: if let Some(html) = breadcrumbs_html { vec![
                proto::PageItem { item: Some(proto::page_item::Item::Html(html.into())) },
                proto::PageItem { item: Some(proto::page_item::Item::FolderListing(folder_listing)) },
            ]} else { vec![
                proto::PageItem { item: Some(proto::page_item::Item::FolderListing(folder_listing)) }
            ]},
        })
}



/// Helper: convert a folder node to a page item.
fn folder_node_to_page_item(folder: &org::PropNode) -> proto::page_item::folder_listing::Item {
    let folder_data = serde_json::from_str::<FolderData>(&folder.body.clone().unwrap_or("{}".into())).unwrap_or_default();
    let f = proto::page_item::folder_listing::Folder {
        id: folder.id.clone(),
        title: if folder_data.name.is_empty() { "<UNNAMED>".into() } else { folder_data.name.clone() },
        preview_items: folder_data.preview_cache,
    };
    proto::page_item::folder_listing::Item {
        item: Some(proto::page_item::folder_listing::item::Item::Folder(f.clone())),
        open_action: Some(proto::ScriptCall {
            lang: proto::script_call::Lang::Javascript.into(),
            code: format!(r#"await call_organizer("open_folder", {{id: "{}"}});"#, f.id),
        }),
        ..Default::default()
    }
}

/// Helper: build breadcrumbs html from folder path.
///
/// Returns None if there is only one item in the path (root folder).
fn make_bredcrumbs_html(folder_path: Vec<org::PropNode>) -> Option<String> {
    let mut breadcrumbs: Vec<(String, String)> = folder_path.iter().map(|f| {
            let id = f.id.clone();
            let name = serde_json::from_str::<FolderData>(&f.body.clone().unwrap_or("{}".into())).unwrap_or_default().name;
            (id, name)
        }).collect();

    if let Some(root) = breadcrumbs.first_mut() { root.1 = "Home".into(); }

    fn format_breadcrumb(id: &String, label: &String, is_last: bool) -> String {
        let args_json = serde_json::to_string(&OpenFolderArgs { id: id.clone() }).unwrap().replace("\"", "'");
        if is_last {
            format!("<strong>{}</stron>", label)
        } else {
            format!(r##"<a style="text-decoration: underline;" href="javascript:clapshot.call_organizer('open_folder', {});">{}</a>"##, args_json, label)
        }
    }

    let breadcrumbs_html = breadcrumbs.iter().enumerate().map(|(idx, (id, label))| {
            let is_last = idx == breadcrumbs.len() - 1;
            format_breadcrumb(id, label, is_last)
        }).collect::<Vec<_>>().join(" ▶ ");

    if breadcrumbs.len() > 1 { Some(breadcrumbs_html) } else { None }
}


// ---------------------------------------------------------------------------

/*
/// Build folder view page.
/// Reads folder_path cookie and builds a list of folders and videos in the folder.
pub async fn _construct_permission_page(_srv: &mut GrpcServerConn, ses: &UserSessionData)
    -> RpcResult<org::ClientShowPageRequest>
{
    // !!! TEMP: read html from file every time for easy development
    // --> replace with include_str!() when done
    let perms_html = std::fs::read_to_string("/home/jarno/clapshot/organizer/default/html/permission_dlg.html")
        .expect("Failed to read html/permission_dlg.html");
    //     //let perms_html = include_str!("../html/permission_dlg.html");

    Ok(org::ClientShowPageRequest {
        sid: ses.sid.clone(),
        page_items: vec![
            proto::PageItem { item: Some(proto::page_item::Item::Html(perms_html.into())) },
        ],
    })
}
*/
