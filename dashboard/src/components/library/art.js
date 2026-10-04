import { coverUrl } from '../../playlistApi.js';

/** The art an item can be shown with: its own, else its album's on this server. */
export const itemArt = (item) => item.artworkUrl || coverUrl(item.coverKey);
