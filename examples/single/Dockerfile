FROM debian:latest

WORKDIR /app

# Handle PID 1 issue with tini
RUN apt update && apt install -y tini

COPY dist/app .
RUN chmod +x /app/app

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["./app"]
