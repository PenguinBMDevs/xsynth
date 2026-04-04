<h1 align="center">XSynth</h1>
<p align="center"><b>A fast Rust-based SoundFont synthesizer designed for high voice counts and low latency.</b></p>
<p align="center">
<img alt="GitHub repo size" src="https://img.shields.io/github/repo-size/BlackMIDIDevs/xsynth">
<img alt="GitHub License" src="https://img.shields.io/github/license/BlackMIDIDevs/xsynth">
<img alt="GitHub Release" src="https://img.shields.io/github/v/release/BlackMIDIDevs/xsynth">
</p>

## Modules

- [`core`](https://github.com/BlackMIDIDevs/xsynth/tree/master/core): Handles the core audio rendering functionality.
- [`clib`](https://github.com/BlackMIDIDevs/xsynth/tree/master/clib): C/C++ bindings for XSynth.
- [`soundfonts`](https://github.com/BlackMIDIDevs/xsynth/tree/master/soundfonts): A module to parse soundfonts to be used in XSynth.
- [`realtime`](https://github.com/BlackMIDIDevs/xsynth/tree/master/realtime): The real-time rendering module within XSynth.
- [`render`](https://github.com/BlackMIDIDevs/xsynth/tree/master/render): Offline rendering support, exposed both as a reusable library and a command line utility for rendering MIDIs to audio using XSynth.
- [`kdmapi`](https://github.com/BlackMIDIDevs/xsynth/tree/master/kdmapi): A cdylib wrapper around XSynth to act as a drop in replacement for OmniMIDI/KDMAPI.

## SF2 Support

XSynth aims for high-performance practical SF2 playback rather than full
SoundFont 2 spec emulation.

Supported today:
- static sample-region playback, including offsets, loop points, stereo links, root key, tuning, scale tuning, key/velocity ranges, fixed key/velocity, filter cutoff/Q, volume envelope generators, and exclusive class
- a baked subset of note-on modulators, resolved at soundfont load time, covering key/velocity sources with linear/concave/convex/switch curves for practical destinations such as attenuation, filter cutoff, pan, volume envelope timings, and static pitch offsets

Intentionally out of scope for performance and binary-size reasons:
- modulation envelope generators and their destinations
- modulation LFO / vibrato LFO generators and destinations
- generic runtime SF2 modulators driven by CCs, aftertouch, or pitch wheel
- SF2 chorus and reverb send behavior

## Demos

#### XSynth playing Immortal Smoke by EpreTroll

https://github.com/user-attachments/assets/d100e3d2-efa0-4367-a774-d5a171ac0bf8

#### XSynth playing DANCE.MID

https://github.com/user-attachments/assets/f509a36c-6019-4d38-9e5e-1bf0eeb9b43d

## License

XSynth and all of its components is licensed under the [GNU Lesser General Public License 3.0](https://www.gnu.org/licenses/lgpl-3.0.en.html#license-text).
