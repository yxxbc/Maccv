//
//  MacCV-Bridging-Header.h
//  MacCV
//
//  Exposes the Rust clipboard engine's C ABI to Swift.
//
//  Swift imports the C declarations directly, so there is no generated Swift
//  wrapper to keep in sync — `maccv_pasteboard_snapshot` and friends appear as
//  ordinary Swift functions.
//

#ifndef MACCV_BRIDGING_HEADER_H
#define MACCV_BRIDGING_HEADER_H

#include "maccv.h"

#endif /* MACCV_BRIDGING_HEADER_H */
