// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package runtime

import (
	"sync"

	"github.com/streamkit/streamkit-codex/plugin-sdk/go/bindings/streamkit/plugin/node"
	"go.bytecodealliance.org/cm"
)

// InstanceRegistry tracks live plugin node instances and hands out the wasm resource handle
// expected by the generated bindings.
type InstanceRegistry[T any] struct {
	mu    sync.Mutex
	next  uint32
	store map[uint32]*T
}

// NewInstanceRegistry constructs an empty registry.
func NewInstanceRegistry[T any]() *InstanceRegistry[T] {
	return &InstanceRegistry[T]{
		next:  1,
		store: make(map[uint32]*T),
	}
}

// Insert stores the provided instance and returns the component-model resource handle that
// should be returned from `node.Exports.NodeInstance.Constructor`.
func (r *InstanceRegistry[T]) Insert(inst *T) node.NodeInstance {
	r.mu.Lock()
	defer r.mu.Unlock()

	handle := r.next
	r.next++
	if r.next == 0 {
		r.next = 1
	}

	r.store[handle] = inst

	rep := cm.Reinterpret[cm.Rep](handle)
	return node.NodeInstanceResourceNew(rep)
}

// Get retrieves a previously registered instance.
func (r *InstanceRegistry[T]) Get(rep cm.Rep) (*T, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()

	handle := cm.Reinterpret[uint32](rep)
	inst, ok := r.store[handle]
	return inst, ok
}

// Remove removes an instance from the registry. This should be wired to the generated destructor.
func (r *InstanceRegistry[T]) Remove(rep cm.Rep) {
	r.mu.Lock()
	defer r.mu.Unlock()

	handle := cm.Reinterpret[uint32](rep)
	delete(r.store, handle)
}
