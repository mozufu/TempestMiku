import {
  initRatex,
  renderLatexToDisplayList,
  renderToCanvas,
} from "./vendor/ratex-wasm/dist/index.js";

const TAG = "miku-ratex-formula";
const DEFAULT_EM = 18;
const DEFAULT_PAD = 8;

class MikuRatexFormulaElement extends HTMLElement {
  static get observedAttributes() {
    return [
      "latex",
      "font-size",
      "padding",
      "background-color",
      "color",
      "display",
    ];
  }

  constructor() {
    super();
    this._canvas = null;
    this._renderToken = 0;
  }

  connectedCallback() {
    if (!this.shadowRoot) {
      const root = this.attachShadow({ mode: "open" });
      root.innerHTML = `
        <style>
          :host {
            display: block;
            width: 100%;
            height: 100%;
            overflow-x: auto;
            overflow-y: hidden;
          }

          .wrap {
            align-items: center;
            display: flex;
            height: 100%;
            min-width: 100%;
          }

          canvas {
            display: block;
            flex: 0 0 auto;
          }
        </style>
        <div class="wrap"><canvas></canvas></div>
      `;
      this._canvas = root.querySelector("canvas");
    }
    initRatex().catch(() => {});
    this._renderWhenReady();
  }

  attributeChangedCallback() {
    this._renderWhenReady();
  }

  async _renderWhenReady() {
    const canvas = this._canvas;
    const latex = (this.getAttribute("latex") || "").trim();
    if (!canvas || !this.isConnected) return;
    if (!latex) {
      canvas.width = 0;
      canvas.height = 0;
      canvas.style.width = "0";
      canvas.style.height = "0";
      return;
    }

    const token = ++this._renderToken;
    try {
      await initRatex();
      if (token !== this._renderToken || !this.isConnected) return;

      const displayList = renderLatexToDisplayList(
        latex,
        this.getAttribute("color") || undefined,
      );
      const cssEm = this._numberAttr("font-size", DEFAULT_EM, 1);
      const cssPad = this._numberAttr("padding", DEFAULT_PAD, 0);
      const dpr = Math.max(1, window.devicePixelRatio || 1);

      renderToCanvas(
        displayList,
        canvas,
        {
          fontSize: cssEm * dpr,
          padding: cssPad * dpr,
          backgroundColor: this.getAttribute("background-color") || "transparent",
        },
        this.getAttribute("color") || undefined,
      );

      canvas.style.width = `${canvas.width / dpr}px`;
      canvas.style.height = `${canvas.height / dpr}px`;
    } catch (err) {
      this._renderError(canvas, latex, err);
    }
  }

  _numberAttr(name, fallback, min) {
    const value = Number(this.getAttribute(name));
    return Number.isFinite(value) && value >= min ? value : fallback;
  }

  _renderError(canvas, latex, err) {
    console.error(`[${TAG}] latex=${JSON.stringify(latex.slice(0, 80))}`, err);
    const dpr = Math.max(1, window.devicePixelRatio || 1);
    const cssWidth = 160;
    const cssHeight = 24;
    canvas.width = cssWidth * dpr;
    canvas.height = cssHeight * dpr;
    canvas.style.width = `${cssWidth}px`;
    canvas.style.height = `${cssHeight}px`;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.scale(dpr, dpr);
    ctx.fillStyle = this.getAttribute("color") || "#ccc";
    ctx.font = "13px sans-serif";
    ctx.fillText("RaTeX error", 0, 17);
  }
}

if (!customElements.get(TAG)) {
  customElements.define(TAG, MikuRatexFormulaElement);
}
