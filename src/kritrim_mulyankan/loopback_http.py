"""Limit resources used by local dashboard and preview HTTP connections."""

from http.server import ThreadingHTTPServer
from threading import BoundedSemaphore


class LoopbackHTTPServer(ThreadingHTTPServer):
    def __init__(self, address, handler):
        super().__init__(address, handler)
        self._slots = BoundedSemaphore(32)

    def process_request(self, request, client_address):
        if not self._slots.acquire(blocking=False):
            self.close_request(request)
            return
        try:
            request.settimeout(15)
            super().process_request(request, client_address)
        except Exception:
            self._slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self._slots.release()
