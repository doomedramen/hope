import { chromium } from "@playwright/test";
import { readFile, writeFile } from "node:fs/promises";

// Keep all browser and home-screen icons derived from the same vector mark.
const publicUrl = new URL("../public/", import.meta.url);
const svg = await readFile(new URL("hope-logo.svg", publicUrl), "utf8");
const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  const images = new Map();
  for (const size of [16, 32, 48, 180, 192, 512]) {
    const data = await page.evaluate(
      async ({ svg, size }) => {
        const image = new Image();
        image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
        await image.decode();
        const canvas = document.createElement("canvas");
        canvas.width = canvas.height = size;
        const context = canvas.getContext("2d");
        // Home-screen icons use an opaque tile; the OS applies its own mask.
        if (size >= 180) {
          context.fillStyle = "#171D26";
          context.fillRect(0, 0, size, size);
        }
        context.drawImage(image, 0, 0, size, size);
        return canvas.toDataURL("image/png").split(",")[1];
      },
      { svg, size },
    );
    images.set(size, Buffer.from(data, "base64"));
  }

  for (const [size, name] of [
    [16, "favicon-16x16.png"],
    [32, "favicon-32x32.png"],
    [180, "apple-touch-icon.png"],
    [192, "icon-192.png"],
    [512, "icon-512.png"],
  ]) {
    await writeFile(new URL(name, publicUrl), images.get(size));
  }
  await writeFile(new URL("favicon.svg", publicUrl), svg);

  // ICO directory with PNG frames at the standard browser sizes.
  const sizes = [16, 32, 48];
  const header = Buffer.alloc(6 + 16 * sizes.length);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(sizes.length, 4);
  let offset = header.length;
  sizes.forEach((size, index) => {
    const entry = 6 + index * 16;
    const image = images.get(size);
    header[entry] = header[entry + 1] = size;
    header.writeUInt16LE(1, entry + 4);
    header.writeUInt16LE(32, entry + 6);
    header.writeUInt32LE(image.length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += image.length;
  });
  await writeFile(
    new URL("favicon.ico", publicUrl),
    Buffer.concat([header, ...sizes.map((size) => images.get(size))]),
  );
} finally {
  await browser.close();
}
