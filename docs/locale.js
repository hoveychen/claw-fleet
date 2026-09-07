/* Choose the first-visit language before painting; explicit links stay explicit. */
(() => {
  const base = new URL('.', document.currentScript.src);
  const current = document.documentElement.lang.startsWith('zh') ? 'zh' : 'en';
  const key = 'claw-fleet-site-language';
  const explicit = new URL(location.href).searchParams.get('lang');
  const valid = (value) => value === 'zh' || value === 'en';
  const remember = (value) => {
    try { localStorage.setItem(key, value); } catch { /* URL still preserves the choice. */ }
  };
  let desired = current;
  if (valid(explicit)) {
    desired = explicit;
    remember(explicit);
  } else if (current === 'en') {
    // A direct /zh/ link is an intentional language choice, independent of browser settings.
    let saved;
    try { saved = localStorage.getItem(key); } catch { /* Storage is optional. */ }
    const preferred = navigator.languages?.[0] || navigator.language || 'en';
    desired = valid(saved) ? saved : (/^zh(?:-|$)/i.test(preferred) ? 'zh' : 'en');
  }
  if (desired !== current) {
    const target = new URL(desired === 'zh' ? 'zh/index.html' : 'index.html', base);
    target.search = location.search;
    target.hash = location.hash;
    location.replace(target.href);
  }
  document.addEventListener('click', (event) => {
    const link = event.target.closest?.('a.language');
    if (!link) return;
    const target = new URL(link.href);
    const lang = target.pathname === new URL('zh/index.html', base).pathname ? 'zh' : 'en';
    // Explicit query works in a new tab and when browser storage is disabled.
    for (const [key, value] of new URL(location.href).searchParams) {
      if (key !== 'lang') target.searchParams.set(key, value);
    }
    target.searchParams.set('lang', lang);
    target.hash = location.hash;
    link.href = target.href;
    remember(lang);
  });
})();
