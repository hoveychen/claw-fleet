import { describe, expect, it } from "vitest";
import { QR_FIXTURE_MATRIX, QR_FIXTURE_URL } from "./scanFrame.fixture";
import { readPairingFromFrame } from "./scanFrame";

/** 把模块矩阵渲染成一帧 RGBA，形状与 `ctx.getImageData()` 交给我们的一致：
 *  每像素 4 字节、逐行、原点左上。`scale` 放大每个模块，`quiet` 是四周的静区
 *  —— 二维码规范要求 4 个模块的静区，没有它多数解码器直接放弃。 */
function renderFrame(matrix: readonly string[], scale: number, quiet: number) {
  const modules = matrix.length;
  const side = (modules + quiet * 2) * scale;
  const data = new Uint8ClampedArray(side * side * 4);
  for (let y = 0; y < side; y++) {
    for (let x = 0; x < side; x++) {
      const mx = Math.floor(x / scale) - quiet;
      const my = Math.floor(y / scale) - quiet;
      const dark =
        my >= 0 && my < modules && mx >= 0 && mx < modules && matrix[my][mx] === "#";
      const v = dark ? 0 : 255;
      const i = (y * side + x) * 4;
      data[i] = v;
      data[i + 1] = v;
      data[i + 2] = v;
      data[i + 3] = 255;
    }
  }
  return { data, width: side, height: side };
}

describe("readPairingFromFrame", () => {
  // 这条是整个扫码入口的地基：fixture 是 qrencode 生成的真二维码，走的是
  // PairScanner 喂给解码器的同一种 RGBA 帧。它绿，说明「摄像头 → canvas →
  // jsQR → 配对链接」这条管线是通的。
  it("从真实二维码位图里解出配对链接，含自建 relay 的 origin", () => {
    const { data, width, height } = renderFrame(QR_FIXTURE_MATRIX, 4, 4);
    const read = readPairingFromFrame(data, width, height);
    expect(read.sawCode).toBe(true);
    expect(read.paired).toEqual({
      secret: QR_FIXTURE_URL.split("#k=")[1],
      relayBase: "https://relay.selfhosted.example",
    });
  });

  it("换一个放大倍数同样解得出（不依赖某个特定分辨率）", () => {
    const { data, width, height } = renderFrame(QR_FIXTURE_MATRIX, 7, 4);
    expect(readPairingFromFrame(data, width, height).paired?.relayBase).toBe(
      "https://relay.selfhosted.example",
    );
  });

  // 实测：静区为 0 —— 二维码顶满整帧 —— jsQR 照样解得出。我原本假设它会失败
  // （规范要求 4 模块静区），实测推翻了。钉住这个结果的意义在于：取景框可以画
  // 得紧，用户把码框满也不会扫不出，不必为此加「请离远一点」之类的提示。
  it("静区为 0 也解得出（实测，与规范要求相反）", () => {
    const { data, width, height } = renderFrame(QR_FIXTURE_MATRIX, 4, 0);
    expect(readPairingFromFrame(data, width, height).paired?.relayBase).toBe(
      "https://relay.selfhosted.example",
    );
  });

  it("纯白画面：既没扫到码，也没有配对链接", () => {
    const side = 64;
    const data = new Uint8ClampedArray(side * side * 4).fill(255);
    expect(readPairingFromFrame(data, side, side)).toEqual({ paired: null, sawCode: false });
  });
});
