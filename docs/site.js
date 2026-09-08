/* All essential copy and GitHub downloads work without this enhancement. */
(() => {
  const script =
    document.currentScript ||
    [...document.scripts].find((el) => /site\.js(?:\?|$)/.test(el.src));
  const manifestURL = new URL("downloads.json", script.src);
  const tabs = [...document.querySelectorAll(".tab")];
  const panels = [...document.querySelectorAll(".demo-panel")];
  const tablist = document.querySelector(".tabs");
  tablist.setAttribute("role", "tablist");
  document.body.classList.add("enhanced");
  function activate(index, focus = false) {
    tabs.forEach((tab, i) => {
      tab.setAttribute("aria-selected", String(i === index));
      tab.tabIndex = i === index ? 0 : -1;
      panels[i].hidden = i !== index;
    });
    if (focus) tabs[index].focus();
  }
  tabs.forEach((tab, i) => {
    tab.setAttribute("role", "tab");
    tab.setAttribute("aria-controls", panels[i].id);
    panels[i].setAttribute("role", "tabpanel");
    panels[i].setAttribute("aria-labelledby", tab.id);
    panels[i].tabIndex = 0;
    tab.addEventListener("click", (event) => {
      event.preventDefault();
      activate(i);
    });
    tab.addEventListener("keydown", (event) => {
      let next;
      if (event.key === "ArrowRight") next = (i + 1) % tabs.length;
      if (event.key === "ArrowLeft") next = (i + tabs.length - 1) % tabs.length;
      if (event.key === "Home") next = 0;
      if (event.key === "End") next = tabs.length - 1;
      if (next !== undefined) {
        event.preventDefault();
        activate(next, true);
      }
    });
  });
  function fromHash() {
    const i = panels.findIndex((panel) => "#" + panel.id === location.hash);
    if (i >= 0) activate(i);
  }
  activate(0);
  fromHash();
  addEventListener("hashchange", fromHash);
  document.querySelectorAll("a.language").forEach((link) =>
    link.addEventListener("click", () => {
      const target = new URL(link.href);
      target.hash = location.hash;
      link.href = target.href;
    }),
  );
  const selector = document.querySelector("#download-source");
  const links = [...document.querySelectorAll("[data-asset]")];
  const originals = new Map(links.map((link) => [link, link.href]));
  const note = document.querySelector("#source-note");
  let mirrorURLs = new Map();
  let sourceChosen = false;
  const changeSource = () =>
    links.forEach((link) => {
      const mirrored = selector.value === "china" && mirrorURLs.has(link);
      link.href = mirrored ? mirrorURLs.get(link) : originals.get(link);
      link.parentElement.querySelector(".fallback").hidden = !mirrored;
    });
  selector.addEventListener("change", changeSource);

  const enableMirror = (manifest, expectedVersion, expectedManifestURL) => {
    if (
      manifest.schema !== 1 ||
      manifest.version !== expectedVersion ||
      !manifest.china
    )
      return false;
    const urls = new Map();
    for (const link of links) {
      const asset = manifest.china.assets?.[link.dataset.asset];
      if (!asset || !/^[a-f0-9]{64}$/.test(asset.sha256)) continue;
      let url;
      try {
        url = new URL(asset.url);
      } catch {
        continue;
      }
      if (url.protocol !== "https:" || url.username || url.password) continue;
      const expectedURL = new URL(
        `releases/${expectedVersion}/${link.dataset.asset}`,
        expectedManifestURL,
      );
      if (url.href !== expectedURL.href) continue;
      urls.set(link, url.href);
    }
    if (urls.size === 0) return false;
    mirrorURLs = urls;
    if (![...selector.options].some((option) => option.value === "china")) {
      selector.add(new Option(note.dataset.china, "china"));
    }
    note.textContent = note.dataset.ready.replace("{version}", expectedVersion);
    if (!sourceChosen) {
      const requestedSource = new URL(location.href).searchParams.get("source");
      if (requestedSource === "china" || requestedSource === "github") {
        selector.value = requestedSource;
      } else if (document.body.dataset.locale === "zh") {
        selector.value = "china";
      }
      sourceChosen = true;
    }
    changeSource();
    return true;
  };

  // The local Pages manifest supplies the GitHub release version. It may point
  // at our first-party Shenzhen manifest so that the mirror appears as soon as
  // it catches up, without another Pages deployment. Any unavailable, stale or
  // malformed mirror leaves the immutable GitHub /releases/latest/ links alone.
  fetch(manifestURL, { signal: AbortSignal.timeout(5000) })
    .then((response) => {
      if (!response.ok) throw new Error("manifest unavailable");
      return response.json();
    })
    .then(async (manifest) => {
      if (manifest.schema !== 1 || typeof manifest.version !== "string") return;
      note.textContent = note.dataset.version.replace(
        "{version}",
        manifest.version,
      );

      if (typeof manifest.mirror_manifest_url !== "string") {
        enableMirror(manifest, manifest.version, manifestURL);
        return;
      }
      let liveURL;
      try {
        liveURL = new URL(manifest.mirror_manifest_url);
      } catch {
        return;
      }
      if (liveURL.protocol !== "https:" || liveURL.username || liveURL.password)
        return;
      enableMirror(manifest, manifest.version, liveURL);
      try {
        const response = await fetch(liveURL, {
          mode: "cors",
          signal: AbortSignal.timeout(5000),
        });
        if (!response.ok) return;
        enableMirror(await response.json(), manifest.version, liveURL);
      } catch {
        /* The GitHub links and version are already ready. */
      }
    })
    .catch(() => {
      /* GitHub remains available. */
    });
})();

// Keep the complete feature catalogue compact until a visitor wants the details.
const catalogueToggle = document.querySelector('.catalogue-toggle');
if (catalogueToggle) catalogueToggle.addEventListener('click', () => {
  const expand = catalogueToggle.getAttribute('aria-expanded') !== 'true';
  document.querySelectorAll('.capability-group').forEach(group => { group.open = expand; });
  catalogueToggle.setAttribute('aria-expanded', String(expand));
  catalogueToggle.textContent = expand ? catalogueToggle.dataset.collapse : catalogueToggle.dataset.expand;
});
