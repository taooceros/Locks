using DataFrames, DataFramesMeta, CSV, Statistics, OnlineStats, Parsers, ProgressMeter, ThreadsX, Chain, Makie, CairoMakie

file = ("visualization/output/response_time_one_three_ratio.csv")

response_time_one_three_ratio = CSV.read(file, DataFrame)

data1 = @chain response_time_one_three_ratio begin
    @subset((:thread_num .== 64), :waiter_type .== "Spin Parker")
end

max_x = quantile(data1[!, :response_time], 0.99)

graphs = @chain data1 begin
    groupby([:locktype, :waiter_type, :job_length, :is_combiner])
    combine([:response_time] => (x -> begin
        ecdf(Float64.(x))(1:10000:max_x)
    end) => :response_time_ecdf)
    groupby([:locktype, :waiter_type, :job_length, :is_combiner])
    @transform(:index = 1:10000:max_x)
end

using AlgebraOfGraphics

CairoMakie.activate!(type="svg")

# graph the ecdf of responsetime layout by (locktype, waiter_type) color by job_length

draw(data(graphs) *
     mapping(:index,
         :response_time_ecdf,
         color=(:job_length, :is_combiner) => ((x, y) -> begin
             join([x, y], ":")
         end),
         layout=:locktype) * (visual(Lines)),
    figure=(size=(1200, 1200),)
 )
