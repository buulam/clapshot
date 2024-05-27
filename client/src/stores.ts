import { writable, type Writable } from 'svelte/store';
import type { IndentedComment, UserMenuItem, MediaProgressReport } from '@/types';
import type { VideoListDefItem } from '@/lib/asset_browser/types';
import * as Proto3 from '@clapshot_protobuf/typescript';

export let videoPlaybackUrl: Writable<string|null> = writable(null);
export let videoOrigUrl: Writable<string|null> = writable(null);
export let mediaFileId: Writable<string|null> = writable(null);
export let videoFps: Writable<number|null> = writable(null);
export let videoTitle: Writable<string|null> = writable("(no video loaded)");
export let latestProgressReports: Writable<MediaProgressReport[]> = writable([]);

export let curPageItems: Writable<Proto3.PageItem[]> = writable([]);
export let curPageId: Writable<string|null> = writable(null);

export let curUsername: Writable<string|null> = writable(null);
export let curUserId: Writable<string|null> = writable(null);
export let curUserIsAdmin: Writable<boolean> = writable(false);
export let curUserPic: Writable<string|null> = writable(null);

export let videoIsReady: Writable<boolean> = writable(false);

export let allComments: Writable<IndentedComment[]> = writable([]);
export let userMessages: Writable<Proto3.UserMessage[]> = writable([]);

export let connectionErrors: Writable<string[]> = writable([]);

export let collabId: Writable<string|null> = writable(null);
export let userMenuItems: Writable<UserMenuItem[]> = writable([]);
export let selectedTiles: Writable<{[key: string]: VideoListDefItem}> = writable({});
export let serverDefinedActions: Writable<{ [key: string]: Proto3.ActionDef }> = writable({});
