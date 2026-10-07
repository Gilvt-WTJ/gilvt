// Injected after mermaid.min.js. The engine calls gilvtRender one request at a time: the theme
// is global mermaid state, and WebKit throttles timers in hidden pages, so a JS-side queue of
// renders would crawl (~2 s each) where fresh calls from the app finish in milliseconds.
let gilvtNext = 0;

async function gilvtRender(src, theme) {
  const id = `gilvt-mermaid-${gilvtNext++}`;
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    htmlLabels: false,
    flowchart: { htmlLabels: false },
    suppressErrorRendering: true,
    theme,
  });
  try {
    const { svg } = await mermaid.render(id, src);
    return svg;
  } finally {
    // mermaid leaves its scratch container behind when parsing fails.
    document.getElementById(`d${id}`)?.remove();
  }
}
