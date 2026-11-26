fetch("https://undeclared.invalid").then(r => postMessage("fetch OPEN " + r.status),
  e => postMessage("fetch rejected (" + e.name + ")"));
