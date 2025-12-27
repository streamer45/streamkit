// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//go:build tinygo.wasm

// Package main provides a Go implementation of the StreamKit gain filter plugin.
// It consumes the shared plugin SDK bindings generated from the StreamKit WIT world.
package main

import (
	"encoding/json"
	"math"
	"strconv"
	"sync"

	"github.com/streamkit/streamkit-codex/plugin-sdk/go/bindings/streamkit/plugin/host"
	"github.com/streamkit/streamkit-codex/plugin-sdk/go/bindings/streamkit/plugin/node"
	"github.com/streamkit/streamkit-codex/plugin-sdk/go/bindings/streamkit/plugin/types"
	"github.com/streamkit/streamkit-codex/plugin-sdk/go/sdk/runtime"
	"go.bytecodealliance.org/cm"
)

const (
	defaultSampleRate = 48_000
	defaultChannels   = 2

	defaultGainDB = float32(0)
	minGainDB     = float32(-60)
	maxGainDB     = float32(20)
)

func okResult() cm.Result[string, struct{}, string] {
	return cm.OK[cm.Result[string, struct{}, string], string, struct{}, string](struct{}{})
}

func errResult(msg string) cm.Result[string, struct{}, string] {
	return cm.Err[cm.Result[string, struct{}, string], string, struct{}, string](msg)
}

type gainParams struct {
	GainDB *float32 `json:"gain_db"`
}

type gainInstance struct {
	mu   sync.RWMutex
	gain float32
}

var instances = runtime.NewInstanceRegistry[gainInstance]()

func init() {
	node.Exports.Metadata = metadata
	node.Exports.NodeInstance.Constructor = constructInstance
	node.Exports.NodeInstance.Process = processPacket
	node.Exports.NodeInstance.UpdateParams = updateParams
	node.Exports.NodeInstance.Cleanup = cleanupInstance
	node.Exports.NodeInstance.Destructor = instances.Remove
}

func metadata() types.NodeMetadata {
	inputPackets := []types.PacketType{
		types.PacketTypeRawAudio(types.AudioFormat{
			SampleRate:   defaultSampleRate,
			Channels:     defaultChannels,
			SampleFormat: types.SampleFormatFloat32,
		}),
	}
	outputPackets := []types.PacketType{
		types.PacketTypeRawAudio(types.AudioFormat{
			SampleRate:   defaultSampleRate,
			Channels:     defaultChannels,
			SampleFormat: types.SampleFormatFloat32,
		}),
	}

	inputs := []types.InputPin{
		{
			Name:         "in",
			AcceptsTypes: cm.ToList(inputPackets),
		},
	}

	outputs := []types.OutputPin{
		{
			Name:         "out",
			ProducesType: outputPackets[0],
		},
	}

	return types.NodeMetadata{
		Kind:        "gain_filter_go",
		Inputs:      cm.ToList(inputs),
		Outputs:     cm.ToList(outputs),
		ParamSchema: gainSchema(),
		Categories:  cm.ToList([]string{"audio", "filters"}),
	}
}

func constructInstance(params cm.Option[string]) node.NodeInstance {
	inst := &gainInstance{}
	if err := inst.applyParams(optionToPtr(params)); err != nil {
		host.Log(host.LogLevelError, "gain_filter: failed to parse params: "+err.Error())
	}
	return instances.Insert(inst)
}

func processPacket(rep cm.Rep, inputPin string, packet types.Packet) cm.Result[string, struct{}, string] {
	inst, ok := instances.Get(rep)
	if !ok {
		return errResult("gain_filter: unknown instance handle")
	}

	if inputPin != "in" {
		return errResult("gain_filter: unexpected input pin")
	}

	audio := packet.Audio()
	if audio == nil {
		return errResult("gain_filter only accepts audio packets")
	}

	gain := inst.currentGain()
	samples := audio.Samples.Slice()
	for i := range samples {
		samples[i] *= gain
	}
	audio.Samples = cm.ToList(samples)

	if sendResult := host.SendOutput("out", types.PacketAudio(*audio)); sendResult.IsErr() {
		errVal := sendResult.Err()
		if errVal != nil {
			return errResult(*errVal)
		}
		return errResult("gain_filter: host send failed")
	}

	return okResult()
}

func updateParams(rep cm.Rep, params cm.Option[string]) cm.Result[string, struct{}, string] {
	inst, ok := instances.Get(rep)
	if !ok {
		return errResult("gain_filter: unknown instance handle")
	}

	if err := inst.applyParams(optionToPtr(params)); err != nil {
		return errResult(err.Error())
	}

	return okResult()
}

func cleanupInstance(rep cm.Rep) {
	if inst, ok := instances.Get(rep); ok {
		host.Log(host.LogLevelInfo, "gain_filter instance shutting down")
		inst.mu.Lock()
		inst.gain = 1
		inst.mu.Unlock()
	}

	instances.Remove(rep)
}

func (i *gainInstance) applyParams(params *string) error {
	gainDB := defaultGainDB

	if params != nil {
		var decoded gainParams
		if err := json.Unmarshal([]byte(*params), &decoded); err != nil {
			return err
		}
		if decoded.GainDB != nil {
			gainDB = clamp(*decoded.GainDB, minGainDB, maxGainDB)
		}
	}

	gainLinear := float32(math.Pow(10, float64(gainDB)/20.0))

	i.mu.Lock()
	i.gain = gainLinear
	i.mu.Unlock()

	host.Log(host.LogLevelInfo, "gain_filter params set to "+formatGain(gainDB, gainLinear))
	return nil
}

func (i *gainInstance) currentGain() float32 {
	i.mu.RLock()
	defer i.mu.RUnlock()

	if i.gain == 0 {
		return 1
	}
	return i.gain
}

func optionToPtr(opt cm.Option[string]) *string {
	return opt.Some()
}

func clamp(val, lo, hi float32) float32 {
	switch {
	case val < lo:
		return lo
	case val > hi:
		return hi
	default:
		return val
	}
}

func gainSchema() string {
	return `{
  "type": "object",
  "properties": {
    "gain_db": {
      "type": "number",
      "default": 0.0,
      "description": "Gain in decibels (dB)",
      "minimum": -60.0,
      "maximum": 20.0
    }
  }
}`
}

func formatGain(db, linear float32) string {
	return formatFloat(db) + "dB (linear: " + formatFloat(linear) + ")"
}

func formatFloat(v float32) string {
	const precision = 3
	scale := float32(math.Pow10(precision))
	rounded := float32(math.Round(float64(v)*float64(scale))) / scale
	return strconv.FormatFloat(float64(rounded), 'f', precision, 64)
}

// TinyGo requires a main entry point for the wasip2 target even if the world
// does not expose it, so provide a stub.
func main() {}
