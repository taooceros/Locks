xmake b example

cd bin

# sudo perf record -a ./example --fc --thread 32 --cpu 32

for cpu in 2 4 8 16 32
do
    sudo perf stat ./example --thread $cpu --cpu $cpu --fc
    sudo perf stat ./example --thread $cpu --cpu $cpu --cc
    sudo perf stat ./example --thread $cpu --cpu $cpu --rcl
done

cpu=32
thread=64

sudo perf stat ./example --thread $thread --cpu $cpu --fc
sudo perf stat ./example --thread $thread --cpu $cpu --cc
sudo perf stat ./example --thread $thread --cpu $cpu --rcl