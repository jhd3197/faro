// File-type icons now live in @faro/file-ui (the open-source UI owns them, so
// there's a single source of truth shared with the file panes). Re-exported
// here so any in-app `@/lib/fileIcons` imports keep resolving unchanged.
export {
  fileIcon,
  isImage,
  imageMime,
  extOf,
  type FileIconSpec,
} from "@faro/file-ui";
