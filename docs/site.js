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
  // No third-party API call. An absent, incomplete or invalid manifest leaves
  // the original working GitHub links in place.
  fetch(manifestURL, { signal: AbortSignal.timeout(5000) })
    .then((response) => {
      if (!response.ok) throw new Error("manifest unavailable");
      return response.json();
    })
    .then((manifest) => {
      if (
        manifest.schema !== 1 ||
        typeof manifest.version !== "string" ||
        !manifest.china
      )
        return;
      const urls = new Map();
      for (const link of links) {
        const asset = manifest.china.assets?.[link.dataset.asset];
        if (!asset || !/^[a-f0-9]{64}$/.test(asset.sha256)) return;
        const url = new URL(asset.url);
        if (url.protocol !== "https:" || url.username || url.password) return;
        urls.set(link, url.href);
      }
      const note = document.querySelector("#source-note");
      selector.add(new Option(note.dataset.china, "china"));
      note.textContent = note.dataset.ready.replace(
        "{version}",
        manifest.version,
      );
      const changeSource = () =>
        links.forEach((link) => {
          link.href =
            selector.value === "china" ? urls.get(link) : originals.get(link);
          link.parentElement.querySelector(".fallback").hidden =
            selector.value !== "china";
        });
      selector.addEventListener("change", changeSource);
      const requestedSource = new URL(location.href).searchParams.get("source");
      if (requestedSource === "china" || requestedSource === "github") {
        selector.value = requestedSource;
      } else if (document.body.dataset.locale === "zh") {
        selector.value = "china";
      }
      changeSource();
    })
    .catch(() => {
      /* GitHub remains available. */
    });
})();
