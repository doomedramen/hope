FROM alpine:3.22
RUN apk add --no-cache python3 iptables iproute2
CMD ["sh", "-c", "iptables -A INPUT -p icmp -j DROP && exec python3 -m http.server 80"]
