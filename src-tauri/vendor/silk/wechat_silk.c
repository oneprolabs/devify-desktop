/*
 * Minimal SILK v3 -> PCM decoder for WeChat voice blobs.
 *
 * The decoder itself is Skype's reference SKP_SILK_SDK (BSD-style licence, see the file
 * headers in this directory). This wrapper is deliberately small: it accepts a memory
 * buffer, strips the WeChat `0x02` prefix and the `#!SILK_V3` magic, then decodes every
 * length-prefixed packet until the buffer ends.
 *
 * WeChat voice blobs never contain simulated packet loss, so the jitter-buffer modelling
 * from the sample decoder is unnecessary; packets are decoded in order and a corrupt or
 * truncated tail simply stops the loop.
 */

#include <stdlib.h>
#include <string.h>

#include "SKP_Silk_SDK_API.h"

#define MAX_INPUT_FRAMES 5

/* The reference SDK declares this hook and expects the embedding application to define it. */
void SKP_assert(int condition)
{
    (void)condition;
}

int wechat_silk_decode(const unsigned char *in, int in_len,
                       short **out_samples, int *out_len, int *out_rate)
{
    const char *magic = "#!SILK_V3";
    int pos = 0;
    int capacity = 0;
    int used = 0;
    short *pcm = NULL;
    short frame[480 * MAX_INPUT_FRAMES];
    SKP_int32 dec_size = 0;
    void *dec = NULL;
    SKP_SILK_SDK_DecControlStruct control;

    if (in == NULL || in_len <= 0 || out_samples == NULL || out_len == NULL || out_rate == NULL) {
        return -1;
    }

    *out_samples = NULL;
    *out_len = 0;
    *out_rate = 0;

    if (in[pos] == 0x02) {
        pos++;
    }
    if (in_len - pos < 9 || memcmp(in + pos, magic, 9) != 0) {
        return -2;
    }
    pos += 9;

    if (SKP_Silk_SDK_Get_Decoder_Size(&dec_size) != SKP_SILK_NO_ERROR) {
        return -3;
    }
    dec = malloc((size_t)dec_size);
    if (dec == NULL) {
        return -4;
    }
    if (SKP_Silk_SDK_InitDecoder(dec) != SKP_SILK_NO_ERROR) {
        free(dec);
        return -5;
    }

    control.API_sampleRate = 24000;
    control.framesPerPacket = 1;

    while (pos + 2 <= in_len) {
        int n_bytes = (int)(signed short)(in[pos] | ((unsigned)in[pos + 1] << 8));
        int frames = 0;
        pos += 2;
        if (n_bytes <= 0 || pos + n_bytes > in_len) {
            break;
        }
        do {
            SKP_int16 produced = 0;
            if (SKP_Silk_SDK_Decode(dec, &control, 0, in + pos, n_bytes, frame, &produced) != SKP_SILK_NO_ERROR) {
                break;
            }
            if (produced < 0) {
                produced = 0;
            }
            if (used + produced > capacity) {
                int next = capacity ? capacity * 2 : 24000;
                short *grown;
                while (next < used + produced) {
                    next *= 2;
                }
                grown = (short *)realloc(pcm, (size_t)next * sizeof(short));
                if (grown == NULL) {
                    free(pcm);
                    free(dec);
                    return -6;
                }
                pcm = grown;
                capacity = next;
            }
            if (produced > 0) {
                memcpy(pcm + used, frame, (size_t)produced * sizeof(short));
                used += produced;
            }
            frames++;
        } while (control.moreInternalDecoderFrames && frames <= MAX_INPUT_FRAMES * 4);
        pos += n_bytes;
    }

    free(dec);

    if (used <= 0) {
        free(pcm);
        return -7;
    }

    *out_samples = pcm;
    *out_len = used;
    *out_rate = 24000;
    return 0;
}

void wechat_silk_free(short *pcm)
{
    free(pcm);
}
