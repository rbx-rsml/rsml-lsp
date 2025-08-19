--!strict 

type Targets<MouseAndKeyboard, Touch, Controller> = {
    MouseAndKeyboard: (MouseAndKeyboard  & true) | (MouseAndKeyboard  & false),
    Touch: (Touch  & true) | (Touch  & false),
    Controller: (Controller  & true) | (Controller  & false),
}

local function Context<MouseAndKeyboard, Touch, Controller>(
    config: {
        Name: string,
        Targets: Targets<MouseAndKeyboard, Touch, Controller>
    }
)
end

type Bindings = {
    MouseAndKeyboard: Enum.KeyCode?,
    --Touch: GuiButton,
    --Controller: (Controller & Enum.KeyCode) | (Controller & nil),
}

local function Action<B>(
    config: {
        Name: string,
        Bindings: Bindings
    }
)
end

Action({
    Name = "Attack",

    Bindings = {
        MouseAndKeyboard = Enum.KeyCode.MouseLeftButton,

    }
})


return {
    Context = Context,
    Action = Action
}


