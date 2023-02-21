#pragma once
#include <memory>
#include <vector>
#include "grpc-libp2p/sql-archives/worker/src/rust_binding.hpp"
#include "grpc-libp2p/src/lib.rs.h"
#include "rust/cxx.h"


namespace subsquid {

    std::unique_ptr<Buffer> newBuffer(size_t size);

    std::unique_ptr<Worker> newWorker();

    std::unique_ptr<MessageSender> wrapSender(rust::Box<P2PSender> sender);

}
