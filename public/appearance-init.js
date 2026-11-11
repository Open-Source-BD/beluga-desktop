// Applies the stored appearance before first paint (external file: the CSP blocks inline scripts).
(function () {
  try {
    var a = localStorage.getItem("appearance");
    if (a === "light" || a === "dark") document.documentElement.setAttribute("data-theme", a);
  } catch (e) {}
})();
