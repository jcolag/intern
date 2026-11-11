# SPDX-FileCopyrightText: 2021 John Colagioia <john@colagioia.net>
#
# SPDX-License-Identifier: AGPL-3.0-or-later

require 'socket'

client = TCPSocket.open 'localhost', 48813
client.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
client.puts ARGV.join(' ')
while line = client.gets do
  puts line.chop
end

client.close

