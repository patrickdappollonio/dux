// Reading a capture: enough of a PNG reader to measure one, and the one rule
// that says a picture is empty.
//
// This is its own module because both halves of the tool need it and they share
// nothing else: the browser scenes measure a pane through lib.js, and the
// terminal UI rasterizer measures the file it just produced. It depends on
// nothing but node, so a scene can be loaded (and the website's loop test can
// load every scene) with this directory's dependencies uninstalled.
const zlib = require("zlib")

// Chromium writes 8-bit, non-interlaced RGB or RGBA, and a screenshot is the
// only picture this ever looks at.
function decodePng(buffer) {
  if (buffer.readUInt32BE(0) !== 0x89504e47) throw new Error("the capture is not a PNG")
  let offset = 8
  let width = 0
  let height = 0
  let depth = 0
  let colorType = 0
  const parts = []
  while (offset + 8 <= buffer.length) {
    const length = buffer.readUInt32BE(offset)
    const type = buffer.toString("ascii", offset + 4, offset + 8)
    const body = buffer.subarray(offset + 8, offset + 8 + length)
    if (type === "IHDR") {
      width = body.readUInt32BE(0)
      height = body.readUInt32BE(4)
      depth = body[8]
      colorType = body[9]
      if (body[12] !== 0) throw new Error("the capture is interlaced")
    } else if (type === "IDAT") {
      parts.push(body)
    } else if (type === "IEND") {
      break
    }
    offset += length + 12
  }
  if (depth !== 8 || (colorType !== 2 && colorType !== 6)) {
    throw new Error(`unsupported capture (bit depth ${depth}, colour type ${colorType})`)
  }
  const channels = colorType === 6 ? 4 : 3
  const raw = zlib.inflateSync(Buffer.concat(parts))
  const stride = width * channels
  // A short stream is the dangerous kind of corrupt: the rows it does not cover
  // stay at the zeroes they were allocated as, which is a picture with black
  // bands in it that every other check here would call fine.
  if (raw.length < height * (stride + 1)) {
    throw new Error(
      `the capture is truncated: ${raw.length} bytes of pixel data for ${height} rows of ${stride}`,
    )
  }
  const pixels = Buffer.alloc(height * stride)
  let previous = Buffer.alloc(stride)
  for (let y = 0; y < height; y++) {
    const at = y * (stride + 1)
    const filter = raw[at]
    const line = raw.subarray(at + 1, at + 1 + stride)
    const out = pixels.subarray(y * stride, (y + 1) * stride)
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? out[i - channels] : 0
      const b = previous[i]
      const c = i >= channels ? previous[i - channels] : 0
      let value
      switch (filter) {
        case 0:
          value = line[i]
          break
        case 1:
          value = line[i] + a
          break
        case 2:
          value = line[i] + b
          break
        case 3:
          value = line[i] + ((a + b) >> 1)
          break
        case 4: {
          const p = a + b - c
          const pa = Math.abs(p - a)
          const pb = Math.abs(p - b)
          const pc = Math.abs(p - c)
          value = line[i] + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)
          break
        }
        default:
          throw new Error(`unknown PNG filter ${filter}`)
      }
      out[i] = value & 0xff
    }
    previous = out
  }
  return { width, height, channels, pixels }
}

// How much of an image is not its own background. Every other pixel in each
// direction is sampled, which is four times less arithmetic and cannot miss a
// glyph: text is many pixels wide at this scale.
const INK_DISTANCE = 24

function inkRatio(image) {
  const { width, height, channels, pixels } = image
  const counts = new Map()
  const sample = []
  for (let y = 0; y < height; y += 2) {
    for (let x = 0; x < width; x += 2) {
      const i = y * width * channels + x * channels
      const key = (pixels[i] << 16) | (pixels[i + 1] << 8) | pixels[i + 2]
      counts.set(key, (counts.get(key) || 0) + 1)
      sample.push(key)
    }
  }
  let background = 0
  let most = -1
  for (const [key, n] of counts) {
    if (n > most) {
      most = n
      background = key
    }
  }
  const br = background >> 16
  const bg = (background >> 8) & 0xff
  const bb = background & 0xff
  let ink = 0
  for (const key of sample) {
    const dr = Math.abs((key >> 16) - br)
    const dg = Math.abs(((key >> 8) & 0xff) - bg)
    const db = Math.abs((key & 0xff) - bb)
    if (Math.max(dr, dg, db) > INK_DISTANCE) ink++
  }
  return { ratio: sample.length ? ink / sample.length : 0, colors: counts.size }
}

// The reading, off any PNG.
const measureInk = (png) => inkRatio(decodePng(png))

// The floor a WHOLE capture has to clear, which is a far weaker claim than any
// guard makes: it only says the picture is not empty. A pure black frame was
// written and reported as a success with every guard passing twice, because
// nothing looked at the artifact; a cropped sidebar with three rows of text in
// it reads a couple of percent, and an empty frame reads zero.
const CAPTURE_INK_FLOOR = 0.001

// What is wrong with this capture, or null. Takes the PNG bytes, so it works the
// same on a buffer a browser handed over and on a file the rasterizer wrote.
function captureProblem(png, { floor = CAPTURE_INK_FLOOR } = {}) {
  const { ratio, colors } = measureInk(png)
  if (colors <= 1) {
    return "the capture is one flat colour; nothing rendered into the picture at all"
  }
  if (ratio < floor) {
    return `the capture is empty: ${(ratio * 100).toFixed(3)}% of its pixels differ from the background (${colors} colours), under the ${(floor * 100).toFixed(3)}% floor`
  }
  return null
}

module.exports = { CAPTURE_INK_FLOOR, captureProblem, decodePng, inkRatio, measureInk }
