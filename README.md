# Shpi

ssh-pipe. For now establishes N tunnels against 1 port listener to accept their inbound payloads. Processes can then watch these payloads and interpret them as seen fit. The initial use case is forwarding notifications, hence `--osc99` flag. 

## Usage

From listener: `shpi start {user@host}`
From remote: `shpi send {message}`
Do something with those messages: `shpi watch [--osc99]`
