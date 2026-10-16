#!/usr/bin/env python3
"""Loopback-only TLS fixture for the Rust server; use an explicit test CA."""
import argparse
import http.client
import http.server
import ssl

parser = argparse.ArgumentParser()
parser.add_argument('--certificate', required=True)
parser.add_argument('--key', required=True)
parser.add_argument('--port', type=int, default=9443)
parser.add_argument('--backend-port', type=int, default=8090)
args = parser.parse_args()

class Proxy(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_):
        pass  # Do not log credentials, headers, installer commands, or request bodies.
    def forward(self):
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if length < 0 or length > 256 * 1024:
                self.send_error(413)
                return
            body = self.rfile.read(length)
            connection = http.client.HTTPConnection('127.0.0.1', args.backend_port, timeout=15)
            hop_headers = {'connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization', 'te', 'trailer', 'transfer-encoding', 'upgrade'}
            headers = {key: value for key, value in self.headers.items() if key.lower() not in hop_headers}
            connection.request(self.command, self.path, body=body, headers=headers)
            response = connection.getresponse()
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in hop_headers:
                    self.send_header(key, value)
            self.send_header('Connection', 'close')
            self.end_headers()
            # read1 returns available bytes, so server-sent events are relayed as they arrive.
            while chunk := response.read1(65536):
                self.wfile.write(chunk)
                self.wfile.flush()
            connection.close()
            self.close_connection = True
        except (OSError, ValueError, http.client.HTTPException):
            self.close_connection = True
    do_GET = do_HEAD = do_POST = do_PUT = do_PATCH = do_DELETE = forward

context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version = ssl.TLSVersion.TLSv1_2
context.load_cert_chain(args.certificate, args.key)
context.set_alpn_protocols(['http/1.1'])
server = http.server.ThreadingHTTPServer(('127.0.0.1', args.port), Proxy)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
