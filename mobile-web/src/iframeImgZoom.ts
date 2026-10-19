// Tap-to-zoom bridge for images living inside a sandboxed <iframe srcDoc> —
// decision-card html previews and wiki docs. The app-level lightbox can't see
// clicks inside the sandbox (opaque origin, no same-origin access), so we inject
// a tiny script that posts the clicked image's src up to the parent, which then
// opens the lightbox. The sandbox keeps `allow-scripts` (needed anyway for the
// height reporter) but NOT `allow-same-origin`, so this runs without granting
// the document DOM/storage access to the parent.
//
// Decision previews served by core already carry core's copy of the bridge
// (`mcp_ipc::IMG_ZOOM_SCRIPT`); the `__fleetImgZoomOn` flag both scripts set
// keeps a document from posting every tap twice.

export { parseImgZoom } from "../../shared-ts/iframeImgZoom";

/** Append to a srcDoc string. Adds a zoom-in cursor and a capture-phase click
 *  listener that posts `{ __fleetImgZoom: <src> }` to the parent window. */
export const IMG_ZOOM_INJECT = `<style>img{cursor:zoom-in}</style><script>
(function(){
if(window.__fleetImgZoomOn)return;window.__fleetImgZoomOn=1;
document.addEventListener('click',function(e){
  var el=e.target;
  while(el&&el.tagName!=='IMG')el=el.parentElement;
  if(el&&el.tagName==='IMG'){
    var src=el.currentSrc||el.src;
    if(src)parent.postMessage({"__fleetImgZoom":src},'*');
  }
},true);
})();
</script>`;
