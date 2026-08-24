/**
 * Attachments in the browser build: the browser's own file dialog standing in
 * for Tauri's native one.
 *
 * The desktop's whole attachment contract is *paths*. `plugin:dialog|open`
 * returns host paths, the composers splice them into the prompt's
 * `Context files:` block, and the agent reads them off disk. A tab cannot
 * produce a host path — a picked `File` is bytes inside the page, and the page
 * is not on the machine the agent runs on even when it looks like it is.
 *
 * So the swap is: pick in the browser, POST the bytes into the host's
 * attachment store, and use the store path as the path. That is the same trade
 * the mobile client already makes (`mobile-web`'s `upload_attachment`), and it
 * is why `webTransport` can keep answering `plugin:dialog|open` with paths
 * instead of every call site growing a second code path.
 *
 * Directories are not covered here and cannot be: there is no browser dialog
 * that yields a *host* directory path. Those call sites switch to
 * `DirPickerDialog`, which browses the backend host over `browse_dir` — the
 * same swap a remote connection already makes.
 */

import { uploadAttachmentBytes } from "./mock/liveProxy";

/**
 * Push each picked file into the host's attachment store, in order, and return
 * the resulting paths.
 *
 * Sequential rather than parallel: the cap is per file, so a multi-select of
 * large images would otherwise put all of them on the wire at once on a
 * connection that may be the reason the user is in a browser at all. A failure
 * rejects rather than returning a short list — a silently dropped attachment
 * reads as "the agent ignored my file", which is the harder bug to see.
 */
export async function uploadPickedFiles(files: File[]): Promise<string[]> {
  const paths: string[] = [];
  for (const file of files) {
    // `File` is a `Blob`, and `fetch` sends a Blob as its bytes verbatim — no
    // need to read it into an ArrayBuffer first (which would double the peak
    // memory for a large image).
    paths.push(await uploadAttachmentBytes(file.name || "attachment.bin", file));
  }
  return paths;
}

/**
 * Open the browser's file dialog and return store paths for what was chosen —
 * the browser build's answer to `plugin:dialog|open` with `directory: false`.
 *
 * `null` is "cancelled", matching the plugin's own contract, and the shape
 * follows `multiple` the way the plugin does (a bare string for a single pick)
 * because the call sites branch on `Array.isArray`.
 *
 * The `<input>` is created, clicked and dropped rather than rendered by a
 * component: this is reached from the transport layer, which has no place in
 * the tree, and the click has to happen inside the user's activation window
 * that the original button press opened.
 */
export async function pickAndUploadFiles(options: {
  multiple?: boolean;
  accept?: string;
}): Promise<string | string[] | null> {
  const files = await chooseFiles(options);
  if (!files || files.length === 0) return null;
  const paths = await uploadPickedFiles(files);
  if (paths.length === 0) return null;
  return options.multiple ? paths : paths[0];
}

/**
 * The browser's file dialog as a promise.
 *
 * Resolves `null` on cancel — but only via the `cancel` event, which is the
 * only reliable signal: a cancelled dialog fires no `change`, so a promise
 * waiting on `change` alone would hang forever and leave the composer's
 * spinner up for the rest of the page's life. `cancel` is in every browser
 * that can run this app; older engines that lack it simply never settle, which
 * is why the element is also detached on either path so it cannot pile up.
 */
function chooseFiles(options: { multiple?: boolean; accept?: string }): Promise<File[] | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    if (options.multiple) input.multiple = true;
    if (options.accept) input.accept = options.accept;
    // Off-screen rather than `display: none`: Safari has historically ignored
    // programmatic clicks on an undisplayed file input.
    input.style.position = "fixed";
    input.style.left = "-9999px";
    input.style.opacity = "0";

    const done = (value: File[] | null) => {
      input.remove();
      resolve(value);
    };
    input.addEventListener("change", () => done([...(input.files ?? [])]));
    input.addEventListener("cancel", () => done(null));

    document.body.appendChild(input);
    input.click();
  });
}
