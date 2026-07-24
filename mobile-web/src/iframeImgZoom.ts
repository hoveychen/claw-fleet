// Tap-to-zoom bridge for images living inside a sandboxed <iframe srcDoc> —
// decision-card html previews and wiki docs. The app-level lightbox can't see
// clicks inside the sandbox (opaque origin, no same-origin access), so we inject
// a tiny script that posts the clicked image's src up to the parent, which then
// opens the lightbox. The sandbox keeps `allow-scripts` (needed anyway for the
// height reporter) but NOT `allow-same-origin`, so this runs without granting
// the document DOM/storage access to the parent.

const ZOOM_KEY = "__fleetImgZoom";

/** Append to a srcDoc string. Adds a zoom-in cursor and a capture-phase click
 *  listener that posts `{ __fleetImgZoom: <src> }` to the parent window. */
export const IMG_ZOOM_INJECT = `<style>img{cursor:zoom-in}</style><script>
document.addEventListener('click',function(e){
  var el=e.target;
  while(el&&el.tagName!=='IMG')el=el.parentElement;
  if(el&&el.tagName==='IMG'){
    var src=el.currentSrc||el.src;
    if(src)parent.postMessage({${JSON.stringify(ZOOM_KEY)}:src},'*');
  }
},true);
</script>`;

/** Pull the image src out of a postMessage payload, or null if it isn't a
 *  zoom message. Validate the shape — the payload crosses a trust boundary. */
export function parseImgZoom(data: unknown): string | null {
  if (!data || typeof data !== "object") return null;
  const src = (data as Record<string, unknown>)[ZOOM_KEY];
  return typeof src === "string" && src.length > 0 ? src : null;
}
