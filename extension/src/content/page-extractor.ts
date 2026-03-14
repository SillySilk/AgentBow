// Injected by service worker to extract page content
(function () {
  function extractPageText(): string {
    // Remove unwanted elements
    const clone = document.body.cloneNode(true) as HTMLElement;
    const unwanted = clone.querySelectorAll(
      "script, style, nav, footer, header, aside, noscript, iframe, [aria-hidden='true']"
    );
    unwanted.forEach((el) => el.remove());

    // Prefer semantic content containers
    const preferred = clone.querySelector("article, main, [role='main']");
    const target = preferred || clone;

    let text = (target as HTMLElement).innerText || "";

    // Truncate at word boundary to ~8000 chars
    if (text.length > 8000) {
      const cut = text.lastIndexOf(" ", 8000);
      text = text.substring(0, cut > 0 ? cut : 8000) + "…";
    }

    return text.trim();
  }

  const result = {
    url: window.location.href,
    title: document.title,
    selectedText: window.getSelection()?.toString() || "",
    pageText: extractPageText(),
  };

  // Return to service worker via scripting.executeScript return value
  return result;
})();
