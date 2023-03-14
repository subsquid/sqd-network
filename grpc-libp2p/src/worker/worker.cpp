#include "grpc-libp2p/src/worker/worker.hpp"

namespace subsquid {

    class WrappedP2PSender: public MessageSender {
        private:
            rust::Box<P2PSender> inner;
        public:
            WrappedP2PSender() = delete;
            WrappedP2PSender(rust::Box<P2PSender> inner) : inner(std::move(inner)) {};
            void sendMessage(const std::string& peerId, std::unique_ptr<Buffer> msg) noexcept {
                inner->sendMessage(peerId, std::move(msg));
            }
    };

    std::unique_ptr<Buffer> newBuffer(size_t size) {
        auto buf = std::make_unique<Buffer>(size);
        return buf;
    }

    std::unique_ptr<Worker> newWorker() {
        return std::make_unique<Worker>();
    }

    std::unique_ptr<MessageSender> wrapSender(rust::Box<P2PSender> sender) {
        return std::make_unique<WrappedP2PSender>(std::move(sender));
    }

}
