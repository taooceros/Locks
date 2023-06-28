using CSV
@time using Plots
@time using DataFrames

@time bench = CSV.read("output/output.csv", DataFrame)

@time begin
    x = range(0, 10, length=100)
    y = sin.(x)
    plot(y)
end