require 'socket'

client = TCPSocket.open 'localhost', 48813
client.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
client.puts ARGV.join(' ')
while line = client.gets do
  puts line.chop
end

client.close

