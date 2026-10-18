using System;
using System.Drawing;
using System.Linq;
using System.Text.Json.Nodes;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace BuddyHub;

public sealed partial class HubForm
{
    readonly ListView shareKeys=List();
    readonly Label shareStatus=new(){AutoSize=true,Font=new Font("Microsoft YaHei UI",15,FontStyle.Bold),ForeColor=TextColor};
    readonly Label shareDetail=new(){AutoSize=true,ForeColor=Muted};
    readonly TextBox shareUrl=new(){Dock=DockStyle.Fill,ReadOnly=true,BorderStyle=BorderStyle.None,BackColor=Color.White,Font=new Font("Consolas",11),AccessibleName="公网 API 地址"};
    readonly CheckBox shareAuto=new(){Text="登录 Windows 后自动开启",AutoSize=true,ForeColor=TextColor};
    Button shareSwitch=null!;
    JsonNode? shareSnapshot;
    bool shareLoading,shareDirty,shareRefreshing;

    Control SharingPage()
    {
        var page=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=1,RowCount=5,Margin=Padding.Empty};
        foreach(var height in new[]{118,102,54})page.RowStyles.Add(new RowStyle(SizeType.Absolute,height));
        page.RowStyles.Add(new RowStyle(SizeType.Percent,100));page.RowStyles.Add(new RowStyle(SizeType.Absolute,44));
        var control=new Surface{Dock=DockStyle.Fill,BackColor=Color.White,Padding=new Padding(20,12,20,8),Margin=new Padding(0,0,0,14)};
        var top=new TableLayoutPanel{Dock=DockStyle.Fill,RowCount=2,ColumnCount=1,Margin=Padding.Empty};
        top.RowStyles.Add(new RowStyle(SizeType.Absolute,36));top.RowStyles.Add(new RowStyle(SizeType.Percent,100));
        shareStatus.Text="正在读取公网服务状态…";top.Controls.Add(shareStatus,0,0);
        var actions=new FlowLayoutPanel{Dock=DockStyle.Fill,WrapContents=false,Margin=Padding.Empty};
        shareSwitch=Btn("开启公网服务",async()=>{await Program.Api("sharing/settings",new{enabled=shareSnapshot?["enabled"]?.GetValue<bool>()!=true});await RefreshSharing();});
        shareAuto.Margin=new Padding(12,10,18,0);shareAuto.CheckedChanged+=(_,_)=>{if(!shareLoading)shareDirty=true;};
        actions.Controls.Add(shareSwitch);actions.Controls.Add(shareAuto);
        actions.Controls.Add(Btn("保存启动设置",async()=>{await Program.Api("sharing/settings",new{autoStart=shareAuto.Checked});shareDirty=false;await RefreshSharing();}));
        actions.Controls.Add(Btn("隧道设置",ConfigureTunnel));top.Controls.Add(actions,0,1);control.Controls.Add(top);page.Controls.Add(control,0,0);
        actions.Controls.Add(Btn("重新连接",async()=>{await Program.Api("sharing/settings",new{enabled=true,restart=true});await RefreshSharing();}));

        var address=new Surface{Dock=DockStyle.Fill,BackColor=Color.White,Padding=new Padding(20,12,20,10),Margin=new Padding(0,0,0,12)};
        var addressRows=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=2,RowCount=2,Margin=Padding.Empty};addressRows.ColumnStyles.Add(new ColumnStyle(SizeType.Percent,100));addressRows.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute,112));addressRows.RowStyles.Add(new RowStyle(SizeType.Absolute,32));addressRows.RowStyles.Add(new RowStyle(SizeType.Percent,100));
        shareUrl.Margin=new Padding(0,6,10,0);addressRows.Controls.Add(shareUrl,0,0);
        var copy=Btn("复制 URL",()=>{var url=(shareSnapshot?["url"]?.ToString()??"").Trim();if(!url.StartsWith("https://"))throw new Exception("公网地址尚未生成，请先开启服务");Clipboard.SetText(url);return Task.CompletedTask;});copy.MinimumSize=new Size(100,30);copy.Height=30;copy.Margin=Padding.Empty;copy.Padding=new Padding(8,2,8,2);addressRows.Controls.Add(copy,1,0);
        addressRows.Controls.Add(shareDetail,0,1);addressRows.SetColumnSpan(shareDetail,2);address.Controls.Add(addressRows);page.Controls.Add(address,0,1);

        var keyActions=Flow();keyActions.WrapContents=false;
        keyActions.Controls.Add(Btn("新增 Key",()=>EditShareKey(null)));
        keyActions.Controls.Add(Btn("编辑与限额",()=>EditShareKey(Selected(shareKeys))));
        keyActions.Controls.Add(Btn("复制 Key",CopyShareKey));
        keyActions.Controls.Add(Btn("复制完整配置",CopyShareConfig));
        keyActions.Controls.Add(Btn("启用 / 停用 Key",async()=>{var k=(JsonObject)Selected(shareKeys).DeepClone();k["enabled"]=k["enabled"]?.GetValue<bool>()!=true;await Program.Api("sharing/key",k);await RefreshSharing();}));
        keyActions.Controls.Add(Btn("刷新用量",RefreshSharing));page.Controls.Add(keyActions,0,2);
        foreach(var (name,width) in new[]{("名称",125),("Key",145),("状态",80),("今日 / 限额",112),("本月 / 限额",112),("累计 / 上限",112),("每分钟",78),("并发",65)})shareKeys.Columns.Add(name,width);
        shareKeys.DoubleClick+=async(_,_)=>await Run(()=>EditShareKey(Selected(shareKeys)));
        var table=new Surface{Dock=DockStyle.Fill,BackColor=Color.White,Padding=new Padding(1),Margin=Padding.Empty};table.Controls.Add(shareKeys);page.Controls.Add(table,0,3);
        page.Controls.Add(new Label{Dock=DockStyle.Fill,ForeColor=Muted,Font=new Font(Font.FontFamily,9),Text="次数包含失败请求；每日 / 每月按北京时间重置，0 表示不限次数。关闭公网不影响本机反代。",Padding=new Padding(0,12,0,0)},0,4);
        return page;
    }
    static string Limit(JsonNode k,string used,string max)=>$"{k[used]??JsonValue.Create(0)} / {(k[max]?.GetValue<long>() is >0?k[max]!.ToString():"不限")}";
    async Task RefreshSharing()
    {
        if(shareRefreshing)return;shareRefreshing=true;
        try
        {
            var snapshot=await Program.Api("sharing");shareSnapshot=snapshot;
            bool enabled=snapshot["enabled"]?.GetValue<bool>()==true;
            bool online=enabled&&snapshot["gatewayRunning"]?.GetValue<bool>()==true&&snapshot["tunnelRunning"]?.GetValue<bool>()==true;
            shareStatus.Text=!enabled?"公网服务已关闭":online?"公网服务已连接":"公网服务未连接";
            shareStatus.ForeColor=online?Accent:TextColor;shareSwitch.Text=enabled?"关闭公网服务":"开启公网服务";
            shareSwitch.BackColor=enabled?Color.White:Accent;shareSwitch.ForeColor=enabled?TextColor:Color.White;
            shareSwitch.FlatAppearance.MouseOverBackColor=enabled?Color.FromArgb(235,241,248):Color.FromArgb(14,129,105);shareSwitch.FlatAppearance.BorderSize=enabled?1:0;
            if(!shareDirty){shareLoading=true;shareAuto.Checked=snapshot["autoStart"]?.GetValue<bool>()==true;shareLoading=false;}
            shareUrl.Text=snapshot["url"]?.ToString() is {Length:>0} url?url:"开启后自动生成 HTTPS 地址";
            shareDetail.Text=snapshot["agentInstalled"]?.GetValue<bool>()!=true?"未安装 ngrok，请先安装官方客户端。":snapshot["tokenConfigured"]?.GetValue<bool>()!=true?"请在“隧道设置”中保存 ngrok Authtoken。":!enabled?"共享后台已停止；重新开启后使用原地址和 Key。":!online?"本机共享入口或隧道尚未连接，可点击“重新连接”。":"免费隧道总额度：每月 20,000 次 HTTP 请求、1 GB 出站流量。";
            Populate(shareKeys,snapshot["keys"]!.AsArray(),k=>new[]{k["name"]?.ToString()??"未命名",k["masked"]?.ToString()??"••••",k["enabled"]?.GetValue<bool>()!=true?"已停用":k["expired"]?.GetValue<bool>()==true?"已到期":"可用",Limit(k,"usedToday","dailyRequests"),Limit(k,"usedMonth","monthlyRequests"),Limit(k,"usedTotal","totalRequests"),k["requestsPerMinute"]?.GetValue<long>() is >0?k["requestsPerMinute"]!.ToString():"不限",k["concurrency"]?.ToString()??"2"});
        }
        catch(Exception e){shareStatus.Text="公网状态读取失败";shareDetail.Text=e.Message;throw;}
        finally{shareRefreshing=false;}
    }
    async Task EditShareKey(JsonNode? key)
    {
        using var dialog=new ShareKeyDialog(key,Font,Bg,TextColor,Muted,Accent);
        if(dialog.ShowDialog(this)!=DialogResult.OK)return;
        var result=await Program.Api("sharing/key",dialog.Value);await RefreshSharing();
        foreach(ListViewItem item in shareKeys.Items)if(((JsonNode)item.Tag!)["id"]?.ToString()==result["id"]?.ToString()){item.Selected=true;item.EnsureVisible();break;}
    }
    async Task CopyShareKey()
    {
        var k=shareKeys.SelectedItems.Count>0?Selected(shareKeys):shareKeys.Items.Count==1?(JsonNode)shareKeys.Items[0].Tag!:throw new Exception("请先选择要复制的 Key");
        var info=await Program.Api("sharing/key-copy",new{id=k["id"]!.ToString()});
        var key=info["key"]?.ToString().Trim()??"";
        if(key.Length==0)throw new Exception("这个 Key 暂时无法读取，请重置后重试");
        Clipboard.SetText(key);
    }
    async Task CopyShareConfig()
    {
        var k=Selected(shareKeys);var info=await Program.Api("sharing/key-copy",new{id=k["id"]!.ToString()});
        var url=info["url"]?.ToString()??"";if(!url.StartsWith("https://"))throw new Exception("请先开启公网服务生成地址");
        var models=info["models"]!.AsArray().Select(m=>m!.ToString()).ToArray();
        var preferred=models.Contains("deepseek-v4.1-flash-auto")?"deepseek-v4.1-flash-auto":models[0];
        Clipboard.SetText($"API 地址：{url}\r\nAPI Key：{info["key"]}\r\n模型：{preferred}\r\n思考强度：high（reasoning_effort）\r\n可用模型：{string.Join("、",models)}");
    }
    async Task ConfigureTunnel()
    {
        using var d=new Form{Text="公网隧道设置",ClientSize=new Size(570,310),StartPosition=FormStartPosition.CenterParent,Font=Font,BackColor=Bg,ForeColor=TextColor,FormBorderStyle=FormBorderStyle.FixedDialog,MaximizeBox=false,MinimizeBox=false};
        var token=new TextBox{Location=new Point(24,60),Width=522,UseSystemPasswordChar=true,PlaceholderText="留空保留已有凭据",AccessibleName="ngrok Authtoken"};
        var size=new NumericUpDown{Location=new Point(240,122),Width=120,Minimum=1,Maximum=128,Value=Math.Clamp(shareSnapshot?["maxBodyMb"]?.GetValue<int>()??64,1,128),AccessibleName="单次请求大小上限 MB"};
        d.Controls.Add(new Label{Text="ngrok Authtoken",Location=new Point(24,28),AutoSize=true});d.Controls.Add(token);
        d.Controls.Add(new Label{Text="单次请求大小上限（MB）",Location=new Point(24,126),AutoSize=true});d.Controls.Add(size);
        d.Controls.Add(new Label{Text="隧道凭据只保存在本机，不会分享给朋友。\r\n更换凭据会重新连接公网服务；本机反代不受影响。",Location=new Point(24,180),Size=new Size(522,55),ForeColor=Muted});
        var save=new Button{Text="保存隧道设置",DialogResult=DialogResult.OK,Location=new Point(390,252),Size=new Size(156,38)};d.Controls.Add(save);d.AcceptButton=save;
        if(d.ShowDialog(this)==DialogResult.OK){await Program.Api("sharing/settings",new{authtoken=token.Text.Trim(),maxBodyMb=(int)size.Value});await RefreshSharing();}
    }
}

sealed class ShareKeyDialog:Form
{
    public JsonObject Value{get;private set;}=new();
    public ShareKeyDialog(JsonNode? key,Font font,Color bg,Color text,Color muted,Color accent)
    {
        Text=key==null?"新增共享 Key":"编辑共享 Key";ClientSize=new Size(620,655);StartPosition=FormStartPosition.CenterParent;Font=font;BackColor=bg;ForeColor=text;FormBorderStyle=FormBorderStyle.FixedDialog;MaximizeBox=false;MinimizeBox=false;
        var grid=new TableLayoutPanel{Location=new Point(24,18),Size=new Size(572,566),ColumnCount=2,RowCount=13};grid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute,170));grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent,100));
        for(int i=0;i<13;i++)grid.RowStyles.Add(new RowStyle(SizeType.Absolute,i is 2 or 10?58:39));
        void Row(int n,string label,Control input){grid.Controls.Add(new Label{Text=label,Dock=DockStyle.Fill,TextAlign=ContentAlignment.MiddleLeft,ForeColor=muted},0,n);input.Dock=DockStyle.Top;input.Margin=new Padding(0,6,0,0);grid.Controls.Add(input,1,n);}
        var name=new TextBox{Text=key?["name"]?.ToString()??"朋友",MaxLength=60,AccessibleName="Key 名称"};Row(0,"名称",name);
        var enabled=new CheckBox{Text="允许使用这个 Key",Checked=key?["enabled"]?.GetValue<bool>()??true,AutoSize=true};Row(1,"状态",enabled);
        var custom=new TextBox{UseSystemPasswordChar=true,PlaceholderText=key==null?"留空自动生成安全 Key":"留空保留原 Key",AccessibleName="自定义共享 Key"};Row(2,"自定义 Key",custom);
        NumericUpDown Number(string field,int fallback,int max,int min=0)=>new(){Minimum=min,Maximum=max,Value=Math.Clamp(key?[field]?.GetValue<decimal>()??fallback,min,max),AccessibleName=field};
        var day=Number("dailyRequests",100,10_000_000);Row(3,"每日次数上限",day);
        var month=Number("monthlyRequests",2000,100_000_000);Row(4,"每月次数上限",month);
        var total=Number("totalRequests",0,1_000_000_000);Row(5,"累计次数上限",total);
        var rpm=Number("requestsPerMinute",10,100_000);Row(6,"每分钟请求数",rpm);
        var concurrency=Number("concurrency",2,16,1);Row(7,"同时进行的请求数",concurrency);
        var expires=key?["expiresAt"]?.GetValue<long>()??0;
        var date=new DateTimePicker{Format=DateTimePickerFormat.Custom,CustomFormat="yyyy-MM-dd HH:mm",ShowCheckBox=true,Value=expires>0?DateTimeOffset.FromUnixTimeMilliseconds(expires).LocalDateTime:DateTime.Now.AddMonths(1),Checked=expires>0,AccessibleName="Key 有效期"};Row(8,"有效期（不勾选为永久）",date);
        var models=new FlowLayoutPanel{Height=38,WrapContents=false};var checks=new[]{new CheckBox{Text="中国版",AutoSize=true},new CheckBox{Text="国际版",AutoSize=true},new CheckBox{Text="双区轮询",AutoSize=true}};
        var ids=new[]{"deepseek-v4.1-flash-cn","deepseek-v4.1-flash-ai","deepseek-v4.1-flash-auto"};
        for(int i=0;i<3;i++){checks[i].Checked=key?["models"] is not JsonArray allowed||allowed.Any(m=>m?.ToString()==ids[i]);models.Controls.Add(checks[i]);}Row(9,"可用模型",models);
        var note=new Label{Text="次数 / 速率填 0 表示不限；并发为 1–16。\r\n失败请求也计数，修改或重置 Key 不清零用量。",Height=54,ForeColor=muted};grid.Controls.Add(note,0,10);grid.SetColumnSpan(note,2);note.Dock=DockStyle.Fill;
        var rotate=new CheckBox{Text="重置 Key（旧 Key 立即失效）",AutoSize=true,Enabled=key!=null};grid.Controls.Add(rotate,0,11);grid.SetColumnSpan(rotate,2);
        Controls.Add(grid);
        var cancel=new Button{Text="取消",DialogResult=DialogResult.Cancel,Location=new Point(368,598),Size=new Size(100,38)};
        var save=new Button{Text="保存 Key",Location=new Point(480,598),Size=new Size(116,38),BackColor=accent,ForeColor=Color.White,FlatStyle=FlatStyle.Flat};save.FlatAppearance.BorderSize=0;Controls.Add(cancel);Controls.Add(save);CancelButton=cancel;AcceptButton=save;
        save.Click+=(_,_)=>{
            if(string.IsNullOrWhiteSpace(name.Text)){MessageBox.Show(this,"请输入 Key 名称。");return;}
            var customValue=custom.Text.Trim();
            if(customValue.Length>0&&(customValue.Length<16||customValue.Length>256||customValue.Any(c=>!char.IsAsciiLetterOrDigit(c)&&!"-_.".Contains(c)))){MessageBox.Show(this,"自定义 Key 需为 16–256 位字母、数字、减号、下划线或点。");return;}
            if(!checks.Any(c=>c.Checked)){MessageBox.Show(this,"请至少选择一个可用模型。");return;}
            var permitted=new JsonArray();for(int i=0;i<3;i++)if(checks[i].Checked)permitted.Add(ids[i]);
            Value=new JsonObject{["id"]=key?["id"]?.ToString()??"",["name"]=name.Text.Trim(),["key"]=custom.Text.Trim(),["enabled"]=enabled.Checked,["rotate"]=rotate.Checked,["dailyRequests"]=(long)day.Value,["monthlyRequests"]=(long)month.Value,["totalRequests"]=(long)total.Value,["requestsPerMinute"]=(long)rpm.Value,["concurrency"]=(long)concurrency.Value,["expiresAt"]=date.Checked?new DateTimeOffset(date.Value).ToUnixTimeMilliseconds():0,["models"]=permitted};
            DialogResult=DialogResult.OK;Close();
        };
    }
}
