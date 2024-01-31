## 
using Arrow, DataFrames, DataFramesMeta, StatsBase, Dates, Makie, ColorSchemes

# Load the compressed data

cd("./visualization")
##

# Plot the data

using Makie, CairoMakie, AlgebraOfGraphics
using AlgebraOfGraphics: density

CairoMakie.activate!(type="svg")

data1 = Arrow.Table("output/counter-proportional-one-to-eight.arrow")

df1 = DataFrame(data1)

function draw_plot(dataset, filename, link_x=false)
    fg = Figure(size=(1200, 1200))

    display(fg)

    save("graphs/$filename.svg", fg)
end

##
df2 = @chain df1 begin
    @subset(:thread_num .== 64, :waiter_type .== "Spin Parker")
    flatten([:is_combiner, :response_time])
    # groupby([:locktype, :waiter_type])
    # @transform(:response_time = (:response_time ./ maximum(:response_time)))
end

freq_plot = data(df2) *
            mapping(:response_time => Dates.value,
                color=:locktype,
                row=:job_length => nonnumeric) * visual(ECDFPlot)

colors = [ColorSchemes.Paired_10[1],
    ColorSchemes.Paired_10[2],
    ColorSchemes.Paired_10[3],
    ColorSchemes.Paired_10[4],
    ColorSchemes.Paired_10[5],
    ColorSchemes.Paired_10[6],
    ColorSchemes.Paired_10[7],
    ColorSchemes.Paired_10[8]]

draw(freq_plot; figure=(; size=(1200, 1200)),
    palettes=(; color=colors))


##

df3 = @chain df2 begin
    @distinct(:id, :locktype)
    dropmissing(:combine_time)
end

combine_time_plt = data(df3) * mapping(:job_length, :combine_time, color=:locktype) * (linear() + visual(Scatter))

draw(combine_time_plt; figure=(; size=(1200, 1200)), palettes=(; color=colors))

##


count_plot = data(df3) * mapping(:job_length, :loop_count, color=:locktype) * (linear() + visual(Scatter))
draw(count_plot)


##

data2 = Arrow.Table("output/response_time_single_addition.arrow")


df3 = DataFrame(data2)

df4 = @chain df3 begin
    dropmissing(:response_time)
    @transform(@byrow :response_time_ecdf = ecdf(:response_time))
end