class ReflexAudioProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buffer = new Float32Array(1024);
    this.offset = 0;
  }

  process(inputs) {
    const input = inputs[0] && inputs[0][0];
    if (!input) return true;

    let cursor = 0;
    while (cursor < input.length) {
      const available = this.buffer.length - this.offset;
      const count = Math.min(available, input.length - cursor);
      this.buffer.set(input.subarray(cursor, cursor + count), this.offset);
      this.offset += count;
      cursor += count;

      if (this.offset === this.buffer.length) {
        const frame = this.buffer;
        this.port.postMessage(frame, [frame.buffer]);
        this.buffer = new Float32Array(1024);
        this.offset = 0;
      }
    }

    return true;
  }
}

registerProcessor("reflex-audio-processor", ReflexAudioProcessor);
