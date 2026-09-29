// Flickering Grid background, adapted from inspira-ui's FlickeringGrid concept
// (https://inspira-ui.com/docs/cn/components/backgrounds/flickering-grid) as a
// small dependency-free canvas implementation for this static launch screen.
(function () {
  const canvas = document.getElementById("flickering-grid");
  if (!canvas || !canvas.getContext) return;

  const ctx = canvas.getContext("2d");

  const SQUARE_SIZE = 4;
  const GRID_GAP = 6;
  const FLICKER_CHANCE = 0.3;
  const MAX_OPACITY = 0.22;
  const COLOR = "77, 107, 254"; // matches --accent

  let dpr = Math.min(window.devicePixelRatio || 1, 2);
  let cols = 0;
  let rows = 0;
  let squares = null;
  let width = 0;
  let height = 0;
  let rafId = null;
  let lastTime = 0;
  let isVisible = true;

  function setupGrid() {
    width = window.innerWidth;
    height = window.innerHeight;
    dpr = Math.min(window.devicePixelRatio || 1, 2);

    canvas.width = Math.floor(width * dpr);
    canvas.height = Math.floor(height * dpr);
    canvas.style.width = width + "px";
    canvas.style.height = height + "px";
    ctx.scale(dpr, dpr);

    const cell = SQUARE_SIZE + GRID_GAP;
    cols = Math.ceil(width / cell);
    rows = Math.ceil(height / cell);
    squares = new Float32Array(cols * rows);
    for (let i = 0; i < squares.length; i++) {
      squares[i] = Math.random() * MAX_OPACITY;
    }
  }

  function draw() {
    ctx.clearRect(0, 0, width, height);
    const cell = SQUARE_SIZE + GRID_GAP;
    for (let j = 0; j < rows; j++) {
      for (let i = 0; i < cols; i++) {
        const opacity = squares[j * cols + i];
        if (opacity <= 0.003) continue;
        ctx.fillStyle = `rgba(${COLOR}, ${opacity.toFixed(3)})`;
        ctx.fillRect(i * cell, j * cell, SQUARE_SIZE, SQUARE_SIZE);
      }
    }
  }

  function update(deltaSeconds) {
    for (let i = 0; i < squares.length; i++) {
      if (Math.random() < FLICKER_CHANCE * deltaSeconds) {
        squares[i] = Math.random() * MAX_OPACITY;
      }
    }
  }

  function loop(time) {
    if (!isVisible) {
      rafId = requestAnimationFrame(loop);
      return;
    }
    const deltaSeconds = lastTime ? (time - lastTime) / 1000 : 0;
    lastTime = time;
    update(Math.min(deltaSeconds, 0.1));
    draw();
    rafId = requestAnimationFrame(loop);
  }

  function start() {
    setupGrid();
    if (rafId === null) {
      rafId = requestAnimationFrame(loop);
    }
  }

  let resizeTimer = null;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(setupGrid, 150);
  });

  document.addEventListener("visibilitychange", () => {
    isVisible = document.visibilityState === "visible";
  });

  start();
})();
