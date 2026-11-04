/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#include <CoreAudio/CoreAudio.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdlib.h>

// Match OpenAL capture's NULL device selection, without opening that device.
// Availability is independent of microphone permission and capture success.
bool touchhle_macos_input_available(void) {
    AudioObjectPropertyAddress address = {
        kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    AudioDeviceID device = kAudioObjectUnknown;
    UInt32 size = sizeof(device);
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL,
                                  &size, &device) != noErr ||
        size != sizeof(device) || device == kAudioObjectUnknown) {
        return false;
    }

    address.mSelector = kAudioDevicePropertyDeviceIsAlive;
    UInt32 alive = 0;
    size = sizeof(alive);
    if (AudioObjectGetPropertyData(device, &address, 0, NULL, &size, &alive) !=
            noErr ||
        size != sizeof(alive) || !alive) {
        return false;
    }

    address.mSelector = kAudioDevicePropertyStreamConfiguration;
    address.mScope = kAudioObjectPropertyScopeInput;
    const size_t header_size = offsetof(AudioBufferList, mBuffers);
    if (AudioObjectGetPropertyDataSize(device, &address, 0, NULL, &size) !=
            noErr ||
        size < header_size) {
        return false;
    }
    const UInt32 capacity = size;
    AudioBufferList *buffers = malloc(capacity);
    if (!buffers) {
        return false;
    }
    bool available = false;
    if (AudioObjectGetPropertyData(device, &address, 0, NULL, &size, buffers) ==
            noErr &&
        size <= capacity && size >= header_size &&
        buffers->mNumberBuffers <= (size - header_size) / sizeof(AudioBuffer)) {
        for (UInt32 i = 0; i < buffers->mNumberBuffers; ++i) {
            if (buffers->mBuffers[i].mNumberChannels != 0) {
                available = true;
                break;
            }
        }
    }
    free(buffers);
    return available;
}
