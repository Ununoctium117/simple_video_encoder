# Simple Video Encoder
This library's goal is to simplify the process of generating a bunch of frames and dumping them into a video file.

This is essentially a wrapper on top of `ffmpeg-sys-next`, with a simplified API useful for (what I hope) is a common task. Video is compressed on the CPU using the common H.264 codec.

See the examples folder for examples.

## Building

In order to build this library, you need to be able to build [`ffmpeg-sys-next`](https://crates.io/crates/ffmpeg-sys-next).

On Windows, this means you must set an environment variable named `FFMPEG_DIR` which points at a directory containing the `lib` folder of an ffmpeg build. You can download a precompiled binary from the [gyan.dev archive](https://github.com/GyanD/codexffmpeg/releases), or build it yourself if you prefer. I've done my testing with version [4.4.1](https://github.com/GyanD/codexffmpeg/releases/tag/4.4.1).

## Input

All input formats are behind feature gates. Currently supported image input formats are:

|Feature Name|Input type|Enabled by default|
|----|----|----|
|`cairo-input`|Cairo surfaces from [`cairo-rs`](https://crates.io/crates/cairo-rs) using the Rgb24 or ARgb24 formats.|No|
|`image-input`|Images from the ubiquitous [`image`](https://crates.io/crates/image) crate.|Yes|

## Output

Because this just calls into ffmpeg, the output container format can be anything that supports H.264 video and which ffmpeg is capable of writing to. The output format is detected automatically using the output file's extension.

## Performance

In my experiments, using this crate is about a 7x to 8x improvement in performance compared to writing all the frames as individual image files to an SSD, and then making the video afterwards with ffmpeg. However, there's probably much that could be done to make this faster. I'm no expert in multimedia programming, and improvements or suggestions are welcome.