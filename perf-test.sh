xmake b example

cd bin

# sudo perf record -a ./example --fc --thread 32 --cpu 32

for cpu in 2 4 8 16 32
do
    ./example --thread $cpu --cpu $cpu --fc
    ./example --thread $cpu --cpu $cpu --fcf
    ./example --thread $cpu --cpu $cpu --fcfpq
    ./example --thread $cpu --cpu $cpu --cc
    ./example --thread $cpu --cpu $cpu --rcl
done

cpu=32
thread=64

./example --thread $thread --cpu $cpu --fc
./example --thread $thread --cpu $cpu --cc
./example --thread $thread --cpu $cpu --rcl