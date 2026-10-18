// The phone's 资料 (library). The merge/filter logic is shared with the desktop
// 资料 page and lives in shared-ts/library.ts; this module pins its generic
// item type to the phone's own WikiDoc / Artifact mirrors.

import type { LibraryItem as GenericLibraryItem } from "../../shared-ts/library";
import type { Artifact, WikiDoc } from "./types";

export * from "../../shared-ts/library";

export type LibraryItem = GenericLibraryItem<WikiDoc, Artifact>;
