import re
from typing import Optional
import json
from html import escape as html_escape

import clapshot_grpc.proto.clapshot as clap
import clapshot_grpc.proto.clapshot.organizer as org
import sqlalchemy

from organizer.database.operations import db_get_or_create_user_root_folder
from organizer.helpers import media_type_to_vis_icon
from organizer.utils import folder_path_to_uri_arg

from .folders import FoldersHelper
from organizer.database.models import DbUser, DbMediaFile, DbFolder


class PagesHelper:
    def __init__(self, folders_helper: FoldersHelper, srv: org.OrganizerOutboundStub, db_new_session: sqlalchemy.orm.sessionmaker, log):
        self.folders_helper = folders_helper
        self.srv = srv
        self.db_new_session = db_new_session
        self.log = log


    async def construct_navi_page(self, ses: org.UserSessionData, cookie_override: Optional[str] = None) -> org.ClientShowPageRequest:
        """
        Construct the main navigation page for given user session.
        """
        folder_path = await self.folders_helper.get_current_folder_path(ses, cookie_override)
        assert len(folder_path) > 0, "Folder path should always contain at least the root folder"

        cur_folder = folder_path[-1]
        parent_folder = folder_path[-2] if len(folder_path) > 1 else None

        pg_items: list[clap.PageItem] = []

        if html := _make_breadcrumbs_html(folder_path):
            pg_items.append(clap.PageItem(html=html))
        else:
            pg_items.append(clap.PageItem(html="<h3>Home</h3>"))

        folder_db_items = await self.folders_helper.fetch_folder_contents(cur_folder, ses)
        pg_items.extend(await self._make_folder_listing(folder_db_items, cur_folder, parent_folder, ses))

        if ses.is_admin and len(folder_path) == 1:
            await self._admin_show_all_user_homes(ses, cur_folder, pg_items)

        page_id = folder_path_to_uri_arg([f.id for f in folder_path])
        return org.ClientShowPageRequest(sid=ses.sid, page_items=pg_items, page_id=page_id, page_title=cur_folder.title)


    async def _admin_show_all_user_homes(self, ses: org.UserSessionData, cur_folder: DbFolder, pg_items: list[clap.PageItem]):
        """
        For each user in the database, show a virtual folder that opens their home folder.
        Admin can also trash all user's content from here.
        """
        pg_items.append(clap.PageItem(html="<h3><strong>ADMIN</strong> – User Folders</h3>"))
        pg_items.append(clap.PageItem(html="<p>The following users currently have a home folder and/or media files.<br/>Uploading files or moving items to these folders will transfer ownership to that user.<br/>Trashing a user's home folder will delete everything they have.</p>"));

        with self.db_new_session() as dbs:
            all_users: list[DbUser] = dbs.query(DbUser).order_by(DbUser.id).distinct().all()

        folders = []
        with self.db_new_session() as dbs:
            for user in all_users:

                if user.id == ses.user.id:
                    continue    # skip self, the view should already show user's own root folder

                users_folder = await db_get_or_create_user_root_folder(dbs, clap.UserInfo(id=user.id, name=user.name), self.srv, self.log)
                assert users_folder, f"User {user.id} has no root folder (should've been autocreated)"

                folders.append(
                    clap.PageItemFolderListingItem(
                        folder = clap.PageItemFolderListingFolder(
                            id = str(users_folder.id),
                            title = user.id,
                            preview_items = []),
                        vis = clap.PageItemFolderListingItemVisualization(
                            icon = clap.Icon(
                                fa_class = clap.IconFaClass(classes="fas fa-user", color=clap.Color(r=184, g=160, b=148)),
                                size = 3.0),
                            base_color = clap.Color(r=160, g=100, b=50)),
                        popup_actions = ["popup_builtin_trash"],   # don't allow any actions on the virtal folders
                        open_action = clap.ScriptCall(
                            lang = clap.ScriptCallLang.JAVASCRIPT,
                            code = f'clapshot.callOrganizer("open_folder", {{id: {users_folder.id}}});'),
                        ))

            user_folder_listing = clap.PageItemFolderListing(
                items = folders,
                popup_actions = [],  # don't allow any actions on this virtual view
                listing_data = {"folder_id": str(cur_folder.id)},
                allow_reordering = False,
                allow_upload = False,
                media_file_added_action = None)

            pg_items.append(clap.PageItem(folder_listing=user_folder_listing))


    async def _make_folder_listing(
            self,
            folder_db_items: list[DbMediaFile | DbFolder],
            cur_folder: DbFolder,
            parent_folder: Optional[DbFolder],
            ses: org.UserSessionData) -> list[clap.PageItem]:
        """
        Make a folder listing for given folder and its contents.
        """
        popup_actions = ["popup_builtin_rename", "popup_builtin_trash"]
        listing_data = {"folder_id": str(cur_folder.id)}

        if parent_folder:
            # If not in root folder, add "move to parent" action to all items
            popup_actions.append("move_to_parent")
            listing_data["parent_folder_id"] = str(parent_folder.id)

        # Fetch media files in this folder
        media_ids = [v.id for v in folder_db_items if isinstance(v, DbMediaFile)]
        media_list = await self.srv.db_get_media_files(org.DbGetMediaFilesRequest(ids=org.IdList(ids=media_ids)))
        media_by_id = {v.id: v for v in media_list.items}

        async def media_file_to_page_item(vid_id: str, popup_actions: list[str]) -> clap.PageItemFolderListingItem:
            assert re.match(r"^[0-9a-fA-F]+$", vid_id), f"Unexpected media file ID format: {vid_id}"
            return clap.PageItemFolderListingItem(
                media_file = media_by_id[vid_id],
                open_action = clap.ScriptCall(
                    lang = clap.ScriptCallLang.JAVASCRIPT,
                    code = f'clapshot.openMediaFile("{vid_id}");'),
                popup_actions = popup_actions,
                vis = media_type_to_vis_icon(media_by_id[vid_id].media_type))

        listing_items: list[clap.PageItemFolderListingItem] = []
        for itm in folder_db_items:
            if isinstance(itm, DbFolder):
                listing_items.append(await self.folders_helper.folder_to_page_item(itm, popup_actions, ses))
            elif isinstance(itm, DbMediaFile):
                listing_items.append(await media_file_to_page_item(itm.id, popup_actions))
            else:
                raise ValueError(f"Unknown item type: {itm}")

        folder_listing = clap.PageItemFolderListing(
            items = listing_items,
            allow_reordering = True,
            popup_actions = ["new_folder"],
            listing_data = listing_data,
            allow_upload = True,
            media_file_added_action = "on_media_file_added")

        pg_items = []
        pg_items.append(clap.PageItem(folder_listing=folder_listing))
        if len(folder_listing.items) == 0:
            pg_items.append(clap.PageItem(html="<p style='margin-top: 1em;'><i class='far fa-circle-question text-blue-400'></i> Use the drop zone to <strong>upload media files</strong>, or right-click on the empty space above to <strong>create a folder</strong>.</p>"))
            pg_items.append(clap.PageItem(html="<p>After that, drag items to <strong>reorder</strong>, or drop them <strong>into folders</strong>. Hold shift to multi-select.</p>"))

        return pg_items


def _make_breadcrumbs_html(folder_path: list[DbFolder]) -> Optional[str]:
    """
    Make a navigation (breadcrumb) trail from the folder path.
    """
    if not folder_path:
        return None

    breadcrumbs: list[tuple[int, str]] = [(f.id, str(f.title or "UNNAMED")) for f in folder_path]
    breadcrumbs[0] = (breadcrumbs[0][0], "[Home]")  # rename root folder to "Home" for UI
    breadcrumbs_html = []

    # Link all but last item
    for (id, title) in breadcrumbs[:-1]:
        args_json = json.dumps({'id': id}).replace('"', "'")
        title = html_escape(title)
        breadcrumbs_html.append(
            f'<a style="text-decoration: underline;" href="javascript:clapshot.callOrganizer(\'open_folder\', {args_json});">{title}</a>')
    # Last item in bold
    for (_, title) in breadcrumbs[-1:]:
        breadcrumbs_html.append(f"<strong>{html_escape(title)}</strong>")

    return ("<h3>" + " ▶ ".join(breadcrumbs_html) + "</h3>") if len(breadcrumbs) > 1 else None
