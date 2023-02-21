using Gadfly
using CSV
using DataFrames
using Pipe

##

rawdatas::Vector{Tuple{String,DataFrame}} = @pipe readdir() |>
                                                  filter(endswith(".csv"), _) |>
                                                  map(x -> (x, CSV.read(x, DataFrame)), _)

for (name, frame::DataFrame) in lockdatas
    frame.locktype .= @pipe name |> replace(_, ".csv" => "")
end

lockdata = @pipe lockdatas |>
                 map(x -> x[2], _) |>
                 reduce(vcat, _)


lockdata.cpu = @pipe lockdata.locktype |>
                     filter.(isdigit, _) |>
                     map(x -> parse(Int, x), _)

plot(lockdata,
    x="lock_acquires",
    x_group="cpu",
    Geom.subplot_grid(Geom.histogram, free_x_axis=true))

##